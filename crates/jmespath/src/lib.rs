//! JMESPath as the AWS CLI evaluates it: `jmespath` 1.0.1 for Python, the
//! library botocore and the CLI import, over any object model that
//! implements [`Runtime`].
//!
//! The lexer, the parser and the interpreter are ports of that library's
//! `lexer.py`, `parser.py`, `visitor.py` and `functions.py`, so that its
//! results and its errors are Python's: a dict keeps its order, an `int`
//! is unbounded and stays an `int` (`sum`, `abs`, `ceil`), `int` and
//! `float` compare exactly, `sum` is CPython 3.14's, `sort` sorts as
//! `list.sort` does, and each exception has its class and its `str()`.
//! What the library takes from Python itself (`json.loads`, `str()`,
//! `json.dumps`, `int()`) the runtime supplies.
//!
//! The Python library is MIT-licensed: Copyright (c) 2013 Amazon.com, Inc.
//! or its affiliates. All Rights Reserved. Its license text is
//! `testdata/python/LICENSE.txt`.
//!
//! Where this differs from Python, by design: an expression reference
//! (`&expr`) is refused where Python would hold it in a value (in an
//! array, an object, `to_string` or the result), since Python's own
//! output of one names its address; and `merge()` of a non-empty array or
//! `bytes` argument, which Python reads as key-value pairs, is refused.
#![forbid(unsafe_code)]

mod error;
mod eval;
mod functions;
mod lexer;
mod number;
mod parser;
mod runtime;
mod sort;
mod subset;

pub use error::Error;
pub use number::{BigInt, Number};
pub use runtime::{Runtime, View};
pub use subset::{Feature, OPERATION_CONTEXT_PARAMS, Outside, Subset};

use std::borrow::Cow;

/// A compiled expression: `jmespath.compile(text)`.
#[derive(Debug)]
pub struct Expression<V> {
    text: Box<str>,
    root: parser::Node<V>,
}

impl<V: Clone> Expression<V> {
    /// `jmespath.compile(text)`: its literals are read with `runtime`.
    ///
    /// # Errors
    /// The `ParseError` (or `LexerError`, `IncompleteExpressionError`,
    /// `EmptyExpressionError`) Python raises.
    pub fn parse<R: Runtime<Value = V>>(runtime: &R, text: &str) -> Result<Self, Error> {
        let root = parser::parse(runtime, text)?;
        Ok(Self {
            text: text.into(),
            root,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// `ParsedResult.search(value)`: borrowed from `value` or the
    /// expression where Python returns an object it was given.
    ///
    /// # Errors
    /// The exception Python raises, and the refusals the crate documents.
    pub fn search<'a, R: Runtime<Value = V>>(
        &'a self,
        runtime: &R,
        value: &'a V,
    ) -> Result<Cow<'a, V>, Error> {
        let interpreter = eval::Interpreter { runtime };
        interpreter.visit(&self.root, value)?.into_cow()
    }

    /// Whether the expression stays within `subset`.
    ///
    /// # Errors
    /// The first construct, outermost first, that it refuses.
    pub fn check(&self, subset: &Subset) -> Result<(), Outside> {
        subset::check(&self.root, subset)
    }
}

/// `jmespath.search(expression, value)`.
///
/// # Errors
/// See [`Expression::parse`] and [`Expression::search`].
pub fn search<R: Runtime>(
    runtime: &R,
    expression: &str,
    value: &R::Value,
) -> Result<R::Value, Error> {
    let expression = Expression::parse(runtime, expression)?;
    Ok(expression.search(runtime, value)?.into_owned())
}
