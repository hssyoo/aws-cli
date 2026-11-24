//! The paginator's shared objects ([`Object`]) as paths of names are read
//! and written through them: [`model::paginators::Expression`]'s
//! `jmespath.search` and `set_value_from_jmespath` over them, and
//! `merge_dicts` (botocore/utils.py), which only the full result uses.
use super::object::Object;
use model::paginators::{Assign, Lookup, NotADict};

impl Lookup for Object {
    fn lookup(&self, name: &str) -> Option<Self> {
        self.get(name)
    }
    fn is_none(&self) -> bool {
        Self::is_none(self)
    }
}

/// Each dict walked is the object itself, so what is set is seen by every
/// other reference to it.
impl Assign for Object {
    type Item = Self;
    fn not_a_dict(&self) -> Option<NotADict<'_>> {
        match self {
            Self::Dict(_) => None,
            Self::List(_) => Some(NotADict::List),
            Self::Value(value) => NotADict::of(value),
        }
    }
    fn child(self, name: &str) -> Option<Self> {
        if let Some(child) = self.get(name) {
            return Some(child);
        }
        let child = Self::dict();
        self.insert(name, child.clone()).ok()?;
        Some(child)
    }
    fn assign(self, name: &str, value: Self) {
        // `not_a_dict` found a dict, which takes any key.
        let _ = self.insert(name, value);
    }
}

/// `merge_dicts(dict1, dict2)`: a dict merges into the dict already at its
/// key, and anything else, the object itself, replaces what is there.
///
/// # Errors
/// The `TypeError` of a dict to merge into something other than a dict.
pub(super) fn merge_dicts(dict1: &Object, dict2: &Object) -> Result<(), String> {
    for (key, value) in dict2.entries() {
        match (&value, dict1.get(&key)) {
            (Object::Dict(_), Some(existing)) => merge_dicts(&existing, &value)?,
            _ => dict1.insert(&key, value)?,
        }
    }
    Ok(())
}
