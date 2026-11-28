//! `TextFormatter.__call__` over a `PageIterator`: each page written as it
//! arrives, the first with the paginator's non-aggregate part, then the
//! token to resume from. The paginator's expressions are the paginators
//! file's ([`Expression`]): member names and dotted paths of them, read as
//! `jmespath` searches them and written as `set_value_from_jmespath` does.
//! `--query` searches each value `format_text` is given
//! (`TextFormatter._format_response`): each page's, and the resume token's.
use crate::Query;
use crate::stream::Encoded;
use crate::text::{PythonError, format_text};
use model::paginators::Expression;
use model::value::{Map, Value};
use pytext::codec::Errors;

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
/// rows after the last page when it is set. Each page is written to a
/// stream encoding with `errors` ([`crate::Destination::errors`]). A page
/// with no rows is no write, as `format_text` makes none for it, so it
/// starts no pager (`LazyStdin`); a page whose first write fails to encode
/// made one. Each value is searched with `query` before it is formatted.
///
/// # Errors
/// See [`Stopped`].
pub fn text<P: Pages>(
    pages: &mut P,
    errors: Errors,
    query: Option<&dyn Query>,
    write: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
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
        let mut out = Encoded::new(errors);
        let failed = text_page(current, &page, pages.result_keys(), query, &mut out);
        written_then(write, out, failed)?;
    }
    if let Some(token) = pages.resume_token().filter(|token| !token.is_empty()) {
        let mut resume = Map::new();
        resume.insert("NextToken".into(), Value::String(token.to_owned()));
        let mut outer = Map::new();
        outer.insert("NextToken".into(), Value::Map(resume));
        let mut out = Encoded::new(errors);
        let failed = queried_text(Value::Map(outer), query, &mut out);
        written_then(write, out, failed)?;
    }
    Ok(())
}

/// What `out` holds handed to `write` once a write was made, then the
/// failure that stopped the formatter after it, if any.
fn written_then<E>(
    write: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
    out: Encoded,
    failed: Option<PythonError>,
) -> Result<(), Stopped<E>> {
    if out.wrote() {
        write(&out.into_bytes()).map_err(Stopped::Write)?;
    }
    failed.map_or(Ok(()), |error| Err(Stopped::Format(error)))
}

/// One page's text, to `out`: each result key's value in `page` set at its
/// expression in `current`, then `current` through `format_text`; the error
/// that stopped it, if any, after the text written before it.
fn text_page(
    current: Map,
    page: &Value,
    result_keys: &[Expression],
    query: Option<&dyn Query>,
    out: &mut Encoded,
) -> Option<PythonError> {
    let mut current = Value::Map(current);
    for key in result_keys {
        let data = key.search(page).cloned().unwrap_or(Value::Null);
        if let Err(message) = key.set(&mut current, data) {
            return Some(PythonError { message });
        }
    }
    queried_text(current, query, out)
}

/// `_format_response`: `value` searched with `query`, then `format_text`
/// of the result; the error that stopped either.
fn queried_text(value: Value, query: Option<&dyn Query>, out: &mut Encoded) -> Option<PythonError> {
    match crate::apply_query(value, query) {
        Ok(value) => format_text(&value, out).err(),
        Err(error) => Some(error),
    }
}

#[cfg(test)]
pub(crate) mod tests;
