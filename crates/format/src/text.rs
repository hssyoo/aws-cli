//! `awscli.text.format_text`: tab-separated rows, scalar keys sorted, each
//! nested structure or list on rows of its own named by its key. Each
//! `stream.write` of `awscli/text.py` is one write to the stream
//! ([`Encoded`]), in `pytext::wtf8`'s encoding, so a lone surrogate it
//! cannot encode fails that write as Python's stream fails it.
use crate::stream::Encoded;
use model::pyrepr::{py_short_type_name, py_str_text};
use model::value::{Key, Map, Value};
use std::borrow::Cow;

/// An exception `format_text` raises, with Python's `str()` of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonError {
    pub message: String,
}

type Written = Result<(), PythonError>;

/// `format_text(data, stream)`, writing to `out`. On an error, `out` holds
/// what Python had written before raising.
pub fn format_text(data: &Value, out: &mut Encoded) -> Written {
    item(data, out, None, None)
}

/// `_format_text`. `identifier` is the key naming the rows, in
/// `pytext::wtf8`'s encoding.
fn item(
    value: &Value,
    out: &mut Encoded,
    identifier: Option<&[u8]>,
    keys: Option<&[&Key]>,
) -> Written {
    match value {
        Value::Map(map) => dict(map, out, identifier, keys),
        Value::List(items) => list(items, out, identifier),
        scalar => {
            out.write(&py_str_text(scalar))?;
            out.write(b"\n")
        }
    }
}

fn is_compound(value: &Value) -> bool {
    matches!(value, Value::Map(_) | Value::List(_))
}

/// `_format_list`.
fn list(items: &[Value], out: &mut Encoded, identifier: Option<&[u8]>) -> Written {
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
            scalar_list(&scalars, out, identifier)?;
        }
        for compound in compounds {
            item(compound, out, identifier, None)?;
        }
    } else {
        scalar_list(&items.iter().collect::<Vec<_>>(), out, identifier)?;
    }
    Ok(())
}

/// `_format_scalar_list`: a row per element under an identifier, each one
/// write, or the bare elements joined, then the newline.
fn scalar_list(elements: &[&Value], out: &mut Encoded, identifier: Option<&[u8]>) -> Written {
    let Some(identifier) = identifier else {
        let texts: Vec<Cow<'_, [u8]>> = elements.iter().map(|value| py_str_text(value)).collect();
        out.write(&texts.join(&b'\t'))?;
        return out.write(b"\n");
    };
    let upper = pytext::wtf8::upper(identifier);
    for element in elements {
        let mut row = upper.clone();
        row.push(b'\t');
        row.extend_from_slice(&py_str_text(element));
        row.push(b'\n');
        out.write(&row)?;
    }
    Ok(())
}

/// `_format_dict`.
fn dict(map: &Map, out: &mut Encoded, identifier: Option<&[u8]>, keys: Option<&[&Key]>) -> Written {
    let (mut scalars, compounds) = partition_dict(map, keys);
    if !scalars.is_empty() {
        if let Some(identifier) = identifier {
            scalars.insert(0, Cow::Owned(pytext::wtf8::upper(identifier)));
        }
        out.write(&scalars.join(&b'\t'))?;
        out.write(b"\n")?;
    }
    for (key, value) in compounds {
        item(value, out, Some(key.as_bytes()), None)?;
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

/// A dict's scalar texts, and its compound values with their keys.
type Partitioned<'a> = (Vec<Cow<'a, [u8]>>, Vec<(&'a Key, &'a Value)>);

/// `_partition_dict`: scalar texts and the compound values with their keys,
/// in key order, or in `keys`' order when a list gave its columns.
fn partition_dict<'a>(map: &'a Map, keys: Option<&[&Key]>) -> Partitioned<'a> {
    let mut entries: Vec<(&Key, &Value)> = map.iter().collect();
    entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let Some(keys) = keys else {
        let (scalars, compounds): (Vec<_>, Vec<_>) =
            entries.into_iter().partition(|(_, v)| !is_compound(v));
        return (
            scalars.into_iter().map(|(_, v)| py_str_text(v)).collect(),
            compounds,
        );
    };
    let scalars = keys
        .iter()
        .map(|key| {
            map.get_key(key)
                .map_or(Cow::Borrowed(&b""[..]), py_str_text)
        })
        .collect();
    let compounds = entries
        .into_iter()
        .filter(|(k, _)| !keys.contains(k))
        .collect();
    (scalars, compounds)
}

#[cfg(test)]
mod tests;
