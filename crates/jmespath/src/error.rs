//! The exceptions `jmespath.search` raises, by class and `str()`: the
//! library's own (`jmespath/exceptions.py`) and the Python errors its
//! functions let through.

/// An exception, as `type(e).__name__` and `str(e)` give it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    class: &'static str,
    message: String,
}

impl Error {
    pub(crate) fn new(class: &'static str, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }

    /// The exception's class name: `ParseError`, `JMESPathTypeError`,
    /// `TypeError`, ...
    pub const fn class(&self) -> &'static str {
        self.class
    }

    /// An exception Python raises that a [`crate::Runtime`] reports.
    pub fn python(class: &'static str, message: &str) -> Self {
        Self::new(class, message)
    }

    /// `str(e)`.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Whether this is one of the library's `JMESPathError`s, as opposed to
    /// a Python error a function let through.
    pub fn is_jmespath_error(&self) -> bool {
        !matches!(self.class, "TypeError" | "ValueError" | "OverflowError")
    }

    pub(crate) fn type_error(message: impl Into<String>) -> Self {
        Self::new("TypeError", message)
    }

    /// `ParseError(position, value, type, reason)` once the parser set its
    /// expression.
    pub(crate) fn parse(
        expression: &str,
        position: usize,
        value: &str,
        kind: &str,
        reason: &str,
    ) -> Self {
        let underline = " ".repeat(position + 1);
        let kind = kind.to_ascii_uppercase();
        Self::new(
            "ParseError",
            format!(
                "{reason}: Parse error at column {position}, token \"{value}\" ({kind}), \
                 for expression:\n\"{expression}\"\n{underline}^"
            ),
        )
    }

    pub(crate) fn incomplete(expression: &str) -> Self {
        let underline = " ".repeat(expression.chars().count() + 1);
        Self::new(
            "IncompleteExpressionError",
            format!(
                "Invalid jmespath expression: Incomplete expression:\n\
                 \"{expression}\"\n{underline}^"
            ),
        )
    }

    pub(crate) fn lexer(expression: &str, position: usize, message: &str) -> Self {
        let underline = " ".repeat(position);
        Self::new(
            "LexerError",
            format!("Bad jmespath expression: {message}:\n{expression}\n{underline}^"),
        )
    }

    pub(crate) fn empty() -> Self {
        Self::new(
            "EmptyExpressionError",
            "Invalid JMESPath expression: cannot be empty.",
        )
    }

    pub(crate) fn arity(expected: usize, actual: usize, name: &str, variadic: bool) -> Self {
        let (class, least) = if variadic {
            ("VariadictArityError", "at least ")
        } else {
            ("ArityError", "")
        };
        let plural = if expected == 1 { "" } else { "s" };
        Self::new(
            class,
            format!(
                "Expected {least}{expected} argument{plural} for function {name}(), \
                 received {actual}"
            ),
        )
    }

    /// `JMESPathTypeError(function, value, actual, expected)`: `value` as
    /// `str()` prints it, `expected` as the list's `repr`.
    pub(crate) fn invalid_type(
        function: &str,
        value: &str,
        actual: &str,
        expected: &[&str],
    ) -> Self {
        let expected: Vec<String> = expected.iter().map(|name| format!("'{name}'")).collect();
        Self::new(
            "JMESPathTypeError",
            format!(
                "In function {function}(), invalid type for value: {value}, \
                 expected one of: [{}], received: \"{actual}\"",
                expected.join(", ")
            ),
        )
    }

    pub(crate) fn unknown_function(name: &str) -> Self {
        Self::new(
            "UnknownFunctionError",
            format!("Unknown function: {name}()"),
        )
    }

    /// What Python's `<` (or another ordering) raises between `left` and
    /// `right`, by their types' names.
    pub(crate) fn unorderable(op: &str, left: &str, right: &str) -> Self {
        Self::type_error(format!(
            "'{op}' not supported between instances of '{left}' and '{right}'"
        ))
    }

    /// A value this evaluator does not hold where Python would: an
    /// expression reference (`&expr`) stored in an array, an object or the
    /// result.
    pub(crate) fn expref_value() -> Self {
        Self::new(
            "UnsupportedError",
            "an expression reference cannot be held in a value",
        )
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}
