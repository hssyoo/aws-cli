//! `awscli.text.format_text`: tab-separated rows, scalar keys sorted, each
//! nested structure or list on rows of its own named by its key.
use model::pyrepr::{py_short_type_name, py_str};
use model::value::{Key, Map, Value};

/// An exception `format_text` raises, with Python's `str()` of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonError {
    pub message: String,
}

type Written = Result<(), PythonError>;

/// `format_text(data, stream)`, writing to `out`. On an error, `out` holds
/// what Python had written before raising.
pub fn format_text(data: &Value, out: &mut String) -> Written {
    item(data, out, None, None)
}

/// `_format_text`.
fn item(
    value: &Value,
    out: &mut String,
    identifier: Option<&str>,
    keys: Option<&[&Key]>,
) -> Written {
    match value {
        Value::Map(map) => dict(map, out, identifier, keys),
        Value::List(items) => list(items, out, identifier),
        scalar => {
            out.push_str(&py_str(scalar));
            out.push('\n');
            Ok(())
        }
    }
}

fn is_compound(value: &Value) -> bool {
    matches!(value, Value::Map(_) | Value::List(_))
}

/// `_format_list`.
fn list(items: &[Value], out: &mut String, identifier: Option<&str>) -> Written {
    if items.is_empty() {
        return Ok(());
    }
    if items.iter().any(|element| matches!(element, Value::Map(_))) {
        let keys = all_scalar_keys(items)?;
        for element in items {
            item(element, out, identifier, Some(&keys))?;
        }
    } else if items
        .iter()
        .any(|element| matches!(element, Value::List(_)))
    {
        let (scalars, compounds): (Vec<&Value>, Vec<&Value>) =
            items.iter().partition(|element| !is_compound(element));
        if !scalars.is_empty() {
            scalar_list(&scalars, out, identifier);
        }
        for compound in compounds {
            item(compound, out, identifier, None)?;
        }
    } else {
        scalar_list(&items.iter().collect::<Vec<_>>(), out, identifier);
    }
    Ok(())
}

/// `_format_scalar_list`.
fn scalar_list(elements: &[&Value], out: &mut String, identifier: Option<&str>) {
    match identifier {
        Some(identifier) => {
            let upper = identifier.to_uppercase();
            for element in elements {
                out.push_str(&format!("{upper}\t{}\n", py_str(element)));
            }
        }
        None => {
            let row: Vec<String> = elements.iter().map(|element| py_str(element)).collect();
            out.push_str(&row.join("\t"));
            out.push('\n');
        }
    }
}

/// `_format_dict`.
fn dict(map: &Map, out: &mut String, identifier: Option<&str>, keys: Option<&[&Key]>) -> Written {
    let (mut scalars, compounds) = partition_dict(map, keys);
    if !scalars.is_empty() {
        if let Some(identifier) = identifier {
            scalars.insert(0, identifier.to_uppercase());
        }
        out.push_str(&scalars.join("\t"));
        out.push('\n');
    }
    for (key, value) in compounds {
        item(value, out, Some(&key.text()), None)?;
    }
    Ok(())
}

/// `_all_scalar_keys`: every element's `.items()`, so an element that is
/// not a dict raises `AttributeError`.
fn all_scalar_keys(items: &[Value]) -> Result<Vec<&Key>, PythonError> {
    let mut keys = Vec::new();
    for element in items {
        let Value::Map(map) = element else {
            return Err(no_items(element));
        };
        keys.extend(map.iter().filter(|(_, v)| !is_compound(v)).map(|(k, _)| k));
    }
    keys.sort_unstable();
    keys.dedup();
    Ok(keys)
}

fn no_items(value: &Value) -> PythonError {
    let name = py_short_type_name(value);
    PythonError {
        message: format!("'{name}' object has no attribute 'items'"),
    }
}

/// `_partition_dict`: scalar texts and the compound values with their keys,
/// in key order, or in `keys`' order when a list gave its columns.
fn partition_dict<'a>(
    map: &'a Map,
    keys: Option<&[&Key]>,
) -> (Vec<String>, Vec<(&'a Key, &'a Value)>) {
    let mut entries: Vec<(&Key, &Value)> = map.iter().collect();
    entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let Some(keys) = keys else {
        let (scalars, compounds): (Vec<_>, Vec<_>) =
            entries.into_iter().partition(|(_, v)| !is_compound(v));
        return (
            scalars.into_iter().map(|(_, v)| py_str(v)).collect(),
            compounds,
        );
    };
    let scalars = keys
        .iter()
        .map(|key| map.get_key(key).map_or_else(String::new, py_str))
        .collect();
    let compounds = entries
        .into_iter()
        .filter(|(k, _)| !keys.contains(k))
        .collect();
    (scalars, compounds)
}

#[cfg(test)]
mod tests;
