//! `TextFormatter.__call__` over a `PageIterator`: each page written as it
//! arrives, the first with the paginator's non-aggregate part, then the
//! token to resume from. The paginator's expressions are member names and
//! dotted paths of them, which [`search`] reads as `jmespath` searches
//! them and [`set`] writes as `set_value_from_jmespath` does.
use crate::text::{PythonError, format_text};
use model::pyrepr::py_type_name;
use model::value::{Map, Value};

/// A paginated response as the text formatter reads a `PageIterator`.
pub trait Pages {
    type Error;
    /// `PageIterator.__iter__`'s next page, or the failure that ends it.
    fn next_page(&mut self) -> Option<Result<Value, Self::Error>>;
    /// `result_keys`, as expressions.
    fn result_keys(&self) -> &[String];
    /// `non_aggregate_part`, which the first page sets.
    fn non_aggregate_part(&self) -> &Map;
    /// `resume_token`, once iteration stopped short of the last page.
    fn resume_token(&self) -> Option<&str>;
}

/// Why writing a paginated response stopped before its end.
#[derive(Debug)]
pub enum Stopped<E> {
    /// A page could not be fetched.
    Page(E),
    /// `format_text` or `set_value_from_jmespath` raised, after the text
    /// it wrote was handed on.
    Format(PythonError),
    /// Writing failed.
    Write(std::io::Error),
}

/// `TextFormatter.__call__` for a paginated response: each page's text
/// handed to `write` as soon as it is formatted, so the output before a
/// failure is what Python had written by then, and the resume token's
/// rows after the last page when it is set. A page with no rows is no
/// write, as `format_text` makes none for it, so it starts no pager
/// (`LazyStdin`).
///
/// # Errors
/// See [`Stopped`].
pub fn text<P: Pages>(
    pages: &mut P,
    write: &mut dyn FnMut(&str) -> std::io::Result<()>,
) -> Result<(), Stopped<P::Error>> {
    let mut first = true;
    while let Some(page) = pages.next_page() {
        let page = page.map_err(Stopped::Page)?;
        let current = if first {
            pages.non_aggregate_part().clone()
        } else {
            Map::new()
        };
        first = false;
        let (written, failed) = text_page(current, &page, pages.result_keys());
        written_then(write, &written, failed)?;
    }
    if let Some(token) = pages.resume_token().filter(|token| !token.is_empty()) {
        let mut resume = Map::new();
        resume.insert("NextToken".into(), Value::String(token.to_owned()));
        let mut outer = Map::new();
        outer.insert("NextToken".into(), Value::Map(resume));
        let (written, failed) = formatted(&Value::Map(outer));
        written_then(write, &written, failed)?;
    }
    Ok(())
}

/// `written` handed to `write` unless it is empty, then the failure that
/// stopped the formatter after it, if any.
fn written_then<E>(
    write: &mut dyn FnMut(&str) -> std::io::Result<()>,
    written: &str,
    failed: Option<PythonError>,
) -> Result<(), Stopped<E>> {
    if !written.is_empty() {
        write(written).map_err(Stopped::Write)?;
    }
    failed.map_or(Ok(()), |error| Err(Stopped::Format(error)))
}

/// One page's text: each result key's value in `page` set at its
/// expression in `current`, then `current` through `format_text`; with the
/// error that stopped it, if any, after the text written before it.
fn text_page(
    mut current: Map,
    page: &Value,
    result_keys: &[String],
) -> (String, Option<PythonError>) {
    for key in result_keys {
        let data = search(page, key).cloned().unwrap_or(Value::Null);
        if let Err(error) = set(&mut current, key, data) {
            return (String::new(), Some(error));
        }
    }
    formatted(&Value::Map(current))
}

fn formatted(value: &Value) -> (String, Option<PythonError>) {
    let mut written = String::new();
    let failed = format_text(value, &mut written).err();
    (written, failed)
}

/// `jmespath.search(expression, value)` for a member name or a dotted
/// path: each name looked up in an object; `None` once one is missing or
/// the value it is looked up in is no object.
fn search<'v>(value: &'v Value, expression: &str) -> Option<&'v Value> {
    expression
        .split('.')
        .try_fold(value, |found, name| match found {
            Value::Map(map) => map.get(name),
            _ => None,
        })
}

/// `set_value_from_jmespath(source, expression, value)`: each name but the
/// last made an empty object where its parent lacks it, and the last
/// assigned. A name whose value is no object raises the `TypeError`
/// Python's `in` or item assignment raises on that value.
///
/// # Errors
/// That `TypeError`, as `str()` gives it.
fn set(source: &mut Map, expression: &str, value: Value) -> Result<(), PythonError> {
    let Some((name, rest)) = expression.split_once('.') else {
        source.insert(expression.to_owned(), value);
        return Ok(());
    };
    if !source.contains_key(name) {
        source.insert(name.to_owned(), Value::Map(Map::new()));
    }
    match source.get_mut(name) {
        Some(Value::Map(child)) => set(child, rest, value),
        Some(other) => Err(not_an_object(other, rest)),
        None => Ok(()),
    }
}

/// What `set_value_from_jmespath(found, rest, ...)` raises when `found` is
/// no dict: with more of the path, `name in found` and then `found[name]`
/// read or assigned; at its end, `found[name] = value`.
fn not_an_object(found: &Value, rest: &str) -> PythonError {
    let class = py_type_name(found);
    let (name, more) = rest
        .split_once('.')
        .map_or((rest, false), |(name, _)| (name, true));
    let message = match found {
        Value::List(_) => "list indices must be integers or slices, not str".to_owned(),
        Value::String(text) if more && text.contains(name) => {
            "string indices must be integers, not 'str'".to_owned()
        }
        Value::Blob(_) if more => "a bytes-like object is required, not 'str'".to_owned(),
        Value::String(_) | Value::Blob(_) => {
            format!("'{class}' object does not support item assignment")
        }
        _ if more => format!("argument of type '{class}' is not a container or iterable"),
        _ => format!("'{class}' object does not support item assignment"),
    };
    PythonError { message }
}

#[cfg(test)]
pub(crate) mod tests;
