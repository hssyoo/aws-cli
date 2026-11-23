//! A paginator expression, a member name or a dotted path of them, as
//! botocore reads and writes one: `jmespath.compile(expression).search`,
//! `set_value_from_jmespath` and `merge_dicts` (botocore/utils.py).
use super::py::type_name;
use model::value::{Map, Value};

/// `jmespath.compile(expression).search(value)`: a field of a non-object
/// is `None`, and so is a field holding `None`.
pub(super) fn search<'v>(value: &'v Value, expression: &str) -> Option<&'v Value> {
    expression
        .split('.')
        .try_fold(value, |current, name| match current {
            Value::Map(map) => map.get(name),
            _ => None,
        })
        .filter(|found| !matches!(found, Value::Null))
}

/// `search`, for a value to change in place.
pub(super) fn search_mut<'v>(value: &'v mut Value, expression: &str) -> Option<&'v mut Value> {
    expression
        .split('.')
        .try_fold(value, |current, name| match current {
            Value::Map(map) => map.get_mut(name),
            _ => None,
        })
}

/// `set_value_from_jmespath(source, expression, value)`: each name but the
/// last a dict, made empty where it is missing.
///
/// # Errors
/// The `TypeError` of a name to set in something other than a dict.
pub(super) fn set(source: &mut Value, expression: &str, value: Value) -> Result<(), String> {
    let (parents, last) = match expression.rsplit_once('.') {
        Some((parents, last)) => (Some(parents), last),
        None => (None, expression),
    };
    let mut current = source;
    for name in parents.into_iter().flat_map(|parents| parents.split('.')) {
        let map = as_dict(current)?;
        if !map.contains_key(name) {
            map.insert(name.to_owned(), Value::Map(Map::new()));
        }
        current = map
            .get_mut(name)
            .ok_or_else(|| format!("'{name}' was just set"))?;
    }
    as_dict(current)?.insert(last.to_owned(), value);
    Ok(())
}

fn as_dict(value: &mut Value) -> Result<&mut Map, String> {
    match value {
        Value::Map(map) => Ok(map),
        other => Err(format!(
            "'{}' object does not support item assignment",
            type_name(other)
        )),
    }
}

/// `merge_dicts(dict1, dict2)`: a dict merges into the dict already at its
/// key, and anything else replaces what is there.
///
/// # Errors
/// The `TypeError` of a dict to merge into something other than a dict.
pub(super) fn merge_dicts(dict1: &mut Map, dict2: &Map) -> Result<(), String> {
    for (key, value) in dict2.iter() {
        match (value, dict1.get_mut(key)) {
            (Value::Map(inner), Some(existing)) => merge_dicts(as_dict(existing)?, inner)?,
            _ => {
                dict1.insert(key.to_owned(), value.clone());
            }
        }
    }
    Ok(())
}
