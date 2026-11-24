//! A paginator expression, a member name or a dotted path of them, as
//! botocore reads and writes one: `jmespath.compile(expression).search`,
//! `set_value_from_jmespath` and `merge_dicts` (botocore/utils.py), over
//! shared [`Object`]s.
use super::object::Object;

/// `jmespath.compile(expression).search(value)`: the object found, a field
/// of a non-dict `None`, and so a field holding `None`.
pub(super) fn search(value: &Object, expression: &str) -> Option<Object> {
    expression
        .split('.')
        .try_fold(value.clone(), |current, name| current.get(name))
        .filter(|found| !found.is_none())
}

/// `set_value_from_jmespath(source, expression, value)`: each name but the
/// last walked into the dict there, an empty one made where it is missing,
/// and the last set in the dict reached.
///
/// # Errors
/// The `TypeError` of a name to set in something other than a dict.
pub(super) fn set(source: &Object, expression: &str, value: Object) -> Result<(), String> {
    let (parents, last) = match expression.rsplit_once('.') {
        Some((parents, last)) => (Some(parents), last),
        None => (None, expression),
    };
    let mut current = source.clone();
    for name in parents.into_iter().flat_map(|parents| parents.split('.')) {
        let child = match current.get(name) {
            Some(child) => child,
            None => {
                let child = Object::dict();
                current.insert(name, child.clone())?;
                child
            }
        };
        current = child;
    }
    current.insert(last, value)
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
