//! The engine's two errors. Each displays the text the CRT engine's
//! `ResolveError` displays for the same failure, so a caller that shows
//! the error keeps today's message; [`DecodeError::detail`] and
//! [`EvalError::detail`] say what went wrong.
use std::fmt;

/// Which document failed to decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Document {
    /// An `endpoint-rule-set-1.json`.
    RuleSet,
    /// A `partitions.json`.
    Partitions,
}

/// A rule set or partitions document the engine refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    document: Document,
    detail: Box<str>,
}

impl DecodeError {
    pub(crate) fn rule_set(detail: impl Into<Box<str>>) -> Self {
        Self {
            document: Document::RuleSet,
            detail: detail.into(),
        }
    }
    /// The document that failed.
    pub fn document(&self) -> Document {
        self.document
    }
    /// What the decoder refused, for diagnostics.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.document {
            Document::RuleSet => "invalid endpoint ruleset",
            Document::Partitions => "invalid endpoint partitions",
        })
    }
}

impl std::error::Error for DecodeError {}

/// An evaluation that ends without an endpoint or a rule's error: a
/// missing required parameter, an argument of the wrong type, no matching
/// rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalError {
    detail: Box<str>,
}

impl EvalError {
    pub(crate) fn new(detail: impl Into<Box<str>>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
    /// Why evaluation failed, for diagnostics.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("endpoint resolution failed")
    }
}

impl std::error::Error for EvalError {}
