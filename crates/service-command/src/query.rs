//! `--query`, as `globalargs._resolve_query` compiles it
//! (`jmespath.compile`) and the formatter searches with it, on the
//! `jmespath` engine over the response's [`Value`]s.
//!
//! An expression the engine could evaluate other than Python does is
//! refused up front, before any request, so that Python runs the command
//! and prints what it prints:
//! - one that does not compile: Python reports its `ParamValidationError`;
//! - one nested deeper than [`NESTING_LIMIT`]: Python's parser and
//!   interpreter recurse, and raise `RecursionError` long before the
//!   engine's stack runs out;
//! - an expression reference anywhere but the argument that `sort_by`,
//!   `min_by`, `max_by` and `map` evaluate it as ([`exprefs_are_arguments`]):
//!   elsewhere Python holds the reference as a value, whose text names its
//!   address;
//! - `merge()`, which Python lets read a list of pairs the engine refuses;
//! - an unknown function name, which Python raises for only once it calls
//!   the function, after what it evaluated before.
//!
//! What the engine still differs in is documented with it, and reaches no
//! response a model parses: NaN identity, and NaN in a sort of 64 or more
//! items. A lone surrogate in the expression is not Unicode, which the
//! parse hands to Python.
use cli_core::cli_error::PythonReason;
use format::PythonError;
use jmespath::{Expression, Feature, Outside, Subset};
use model::pyjmespath::Python;
use model::value::Value;

/// A compiled `--query`.
#[derive(Debug)]
pub struct Query(Expression<Value>);

/// The deepest nesting admitted: the count of the characters that open a
/// level (`(`, `[`, `{`, `!`, `&`), literals included. Python's default
/// recursion limit of 1000 frames, with about four frames a level, stays
/// out of reach.
pub const NESTING_LIMIT: usize = 100;

/// Every construct of the language but expression references.
const WITHOUT_EXPREF: &[Feature] = &[
    Feature::Identifier,
    Feature::Subexpression,
    Feature::Index,
    Feature::Slice,
    Feature::ListWildcard,
    Feature::ObjectWildcard,
    Feature::Filter,
    Feature::Flatten,
    Feature::Current,
    Feature::Literal,
    Feature::MultiSelectList,
    Feature::MultiSelectHash,
    Feature::Logic,
    Feature::Pipe,
    Feature::Comparator,
    Feature::Function,
];

/// Every construct of the language.
const ALL_FEATURES: &[Feature] = &[
    Feature::Identifier,
    Feature::Subexpression,
    Feature::Index,
    Feature::Slice,
    Feature::ListWildcard,
    Feature::ObjectWildcard,
    Feature::Filter,
    Feature::Flatten,
    Feature::Current,
    Feature::Literal,
    Feature::MultiSelectList,
    Feature::MultiSelectHash,
    Feature::Logic,
    Feature::Pipe,
    Feature::Comparator,
    Feature::Function,
    Feature::ExpressionReference,
];

/// jmespath 1.0.1's functions but `merge`.
const FUNCTIONS: &[&str] = &[
    "abs",
    "avg",
    "ceil",
    "contains",
    "ends_with",
    "floor",
    "join",
    "keys",
    "length",
    "map",
    "max",
    "max_by",
    "min",
    "min_by",
    "not_null",
    "reverse",
    "sort",
    "sort_by",
    "starts_with",
    "sum",
    "to_array",
    "to_number",
    "to_string",
    "type",
    "values",
];

/// The language as the native route evaluates `--query`.
const QUERY: Subset = Subset {
    features: ALL_FEATURES,
    functions: FUNCTIONS,
};

/// [`QUERY`] without expression references.
const WITHOUT_EXPREFS: Subset = Subset {
    features: WITHOUT_EXPREF,
    functions: FUNCTIONS,
};

/// The functions that evaluate an expression reference, and the argument
/// it is.
const EXPREF_ARGUMENTS: &[(&str, usize)] =
    &[("sort_by", 1), ("min_by", 1), ("max_by", 1), ("map", 0)];

impl Query {
    /// `jmespath.compile(text)`, within what the native route evaluates.
    ///
    /// # Errors
    /// The command is Python's: the expression does not compile, or is
    /// outside what the native route evaluates as Python does.
    pub fn compile(text: &str) -> Result<Self, PythonReason> {
        let refused = PythonReason::PythonOnlyGlobal;
        if nesting(text) > NESTING_LIMIT {
            return Err(refused);
        }
        let expression = Expression::parse(&Python, text).map_err(|_| refused)?;
        expression.check(&QUERY).map_err(|_| refused)?;
        match expression.check(&WITHOUT_EXPREFS) {
            Ok(()) => {}
            Err(Outside::Feature(Feature::ExpressionReference)) if exprefs_are_arguments(text) => {}
            Err(_) => return Err(refused),
        }
        Ok(Self(expression))
    }
}

impl format::Query for Query {
    fn search(&self, value: &Value) -> Result<Value, PythonError> {
        match self.0.search(&Python, value) {
            Ok(found) => Ok(found.into_owned()),
            Err(error) => Err(PythonError {
                message: error.message().to_owned(),
            }),
        }
    }
}

/// The count of characters in `text` that open a level of nesting.
fn nesting(text: &str) -> usize {
    text.bytes().filter(|byte| b"([{!&".contains(byte)).count()
}

/// A level the scan of [`exprefs_are_arguments`] is in.
enum Level<'a> {
    /// A function call's parentheses: its name, and the argument reached.
    Call(&'a str, usize),
    /// Any other parentheses, brackets or braces.
    Other,
}

/// What [`exprefs_are_arguments`] reads of the text: the tokens that open
/// and close levels, separate arguments, name a call, or are an `&`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    /// An unquoted identifier.
    Name(&'a str),
    /// A lone `&`.
    Expref,
    /// `(`.
    Paren,
    /// `[` or `{`.
    Open,
    /// `)`, `]` or `}`.
    Close,
    Comma,
    /// Anything else: `&&`, an operator, a literal, a raw string, a
    /// quoted identifier, a number.
    Other,
}

/// The tokens of `text`, whitespace skipped.
fn tokens(text: &str) -> Vec<Token<'_>> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let (token, next) = token_at(text, i);
        tokens.extend(token);
        i = next;
    }
    tokens
}

/// The token that starts at `i`, if it is not whitespace, and the index
/// past it.
fn token_at(text: &str, i: usize) -> (Option<Token<'_>>, usize) {
    let bytes = text.as_bytes();
    let token = match bytes[i] {
        b'\'' | b'`' | b'"' => return (Some(Token::Other), skip_delimited(bytes, i)),
        b' ' | b'\t' | b'\n' | b'\r' => return (None, i + 1),
        b'&' if bytes.get(i + 1) == Some(&b'&') => return (Some(Token::Other), i + 2),
        b'&' => Token::Expref,
        b'(' => Token::Paren,
        b'[' | b'{' => Token::Open,
        b')' | b']' | b'}' => Token::Close,
        b',' => Token::Comma,
        byte if byte.is_ascii_alphabetic() || byte == b'_' => {
            let end = bytes[i..]
                .iter()
                .position(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
                .map_or(bytes.len(), |length| i + length);
            return (text.get(i..end).map(Token::Name), end);
        }
        _ => Token::Other,
    };
    (Some(token), i + 1)
}

/// Whether every expression reference in `text` is a whole argument that
/// a function evaluates as one ([`EXPREF_ARGUMENTS`]): an `&` that is not
/// `&&`, right after the call's `(` or a `,` of its own, the call named by
/// an unquoted identifier. Literals, raw strings and quoted identifiers
/// are skipped. An expression reference takes everything up to the
/// argument's end (its binding power is 0), so a reference so placed is
/// the argument.
fn exprefs_are_arguments(text: &str) -> bool {
    let mut levels: Vec<Level<'_>> = Vec::new();
    let mut previous = Token::Other;
    for token in tokens(text) {
        match token {
            Token::Expref if !placed(previous, levels.last()) => return false,
            Token::Paren => levels.push(match previous {
                Token::Name(name) => Level::Call(name, 0),
                _ => Level::Other,
            }),
            Token::Open => levels.push(Level::Other),
            Token::Close => {
                levels.pop();
            }
            Token::Comma => {
                if let Some(Level::Call(_, index)) = levels.last_mut() {
                    *index += 1;
                }
            }
            _ => {}
        }
        previous = token;
    }
    true
}

/// Whether an `&` after `previous`, in `level`, is an argument its
/// function evaluates.
fn placed(previous: Token<'_>, level: Option<&Level<'_>>) -> bool {
    matches!(previous, Token::Paren | Token::Comma)
        && matches!(level, Some(Level::Call(name, index))
            if EXPREF_ARGUMENTS.contains(&(*name, *index)))
}

/// The index past the literal, raw string or quoted identifier that starts
/// at `start`: its closing delimiter is the next one no backslash escapes.
fn skip_delimited(bytes: &[u8], start: usize) -> usize {
    let delimiter = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            byte if byte == delimiter => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

#[cfg(test)]
mod tests;
