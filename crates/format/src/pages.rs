//! `TextFormatter.__call__` over a `PageIterator`: each page written as it
//! arrives, the first with the paginator's non-aggregate part, then the
//! token to resume from. The paginator's expressions are the paginators
//! file's ([`Expression`]): member names and dotted paths of them, read as
//! `jmespath` searches them and written as `set_value_from_jmespath` does.
use crate::text::{PythonError, format_text};
use model::paginators::Expression;
use model::value::{Map, Value};

/// A paginated response as the text formatter reads a `PageIterator`.
pub trait Pages {
    type Error;
    /// `PageIterator.__iter__`'s next page, or the failure that ends it.
    fn next_page(&mut self) -> Option<Result<Value, Self::Error>>;
    /// `result_keys`, as expressions.
    fn result_keys(&self) -> &[Expression];
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
    current: Map,
    page: &Value,
    result_keys: &[Expression],
) -> (String, Option<PythonError>) {
    let mut current = Value::Map(current);
    for key in result_keys {
        let data = key.search(page).cloned().unwrap_or(Value::Null);
        if let Err(message) = key.set(&mut current, data) {
            return (String::new(), Some(PythonError { message }));
        }
    }
    formatted(&current)
}

fn formatted(value: &Value) -> (String, Option<PythonError>) {
    let mut written = String::new();
    let failed = format_text(value, &mut written).err();
    (written, failed)
}

#[cfg(test)]
pub(crate) mod tests;
