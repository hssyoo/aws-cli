//! `--query`, as `globalargs._resolve_query` compiles it
//! (`jmespath.compile`) and the formatter searches with it
//! (`ParsedResult.search`), over the response's [`Value`]s.
//!
//! The expression is compiled as a paginator's is
//! ([`model::paginators::Expression`]): a dotted path of names, or any
//! expression within [`model::paginators::PAGINATORS`], every construct of
//! which the `jmespath` engine evaluates without raising, as Python does.
//! Anything else is refused before any request, so that Python runs the
//! command and prints what it prints: an expression that does not compile,
//! whose `ParamValidationError` Python reports; one nested deeper than
//! [`NESTING_LIMIT`]; and functions, comparators, slices and expression
//! references, which this crate cannot evaluate without depending on the
//! engine itself.
use cli_core::cli_error::PythonReason;
use format::PythonError;
use model::paginators::Expression;
use model::value::Value;

/// A compiled `--query`.
#[derive(Debug)]
pub struct Query(Expression);

/// The deepest nesting admitted: the count of the characters that can open
/// a level (`(`, `[`, `{`, `!`, `&`), literals included. Python's parser
/// and interpreter recurse, with a few frames a level, so its default
/// recursion limit of 1000 frames stays out of reach; so does the native
/// engine's stack.
pub const NESTING_LIMIT: usize = 100;

impl Query {
    /// `jmespath.compile(text)`, within what the native route evaluates.
    ///
    /// # Errors
    /// The command is Python's: the expression does not compile, or is
    /// outside what the native route evaluates.
    pub fn compile(text: &str) -> Result<Self, PythonReason> {
        if nesting(text) > NESTING_LIMIT {
            return Err(PythonReason::PythonOnlyGlobal);
        }
        Expression::parse(text)
            .map(Self)
            .ok_or(PythonReason::PythonOnlyGlobal)
    }
}

impl format::Query for Query {
    /// Every expression [`Query::compile`] admits evaluates without
    /// raising.
    fn search(&self, value: &Value) -> Result<Value, PythonError> {
        Ok(self.0.find(value))
    }
}

/// The count of characters in `text` that can open a level of nesting.
fn nesting(text: &str) -> usize {
    text.bytes().filter(|byte| b"([{!&".contains(byte)).count()
}

#[cfg(test)]
mod tests;
