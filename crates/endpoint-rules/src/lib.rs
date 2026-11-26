//! The endpoint rules engine: decodes an `endpoint-rule-set-1.json` once
//! into a [`Program`] whose names are scope slots and whose functions are
//! resolved, and evaluates it for a call's parameters to an endpoint or a
//! rule's error.
//!
//! Its semantics are the CRT engine's (`aws-c-sdkutils`), which the
//! client resolves with today, quirks included: an unset or mistyped
//! function argument fails evaluation, a matched tree rule is final, only
//! a required parameter takes its default, and template text after the
//! last `{` is kept as written. Its errors display that engine's texts.
//!
//! ```
//! use endpoint_rules::{EvalValue, Library, Outcome, Params, Partitions, Program};
//! let rules = br#"{"version": "1.0",
//!     "parameters": {"Region": {"type": "String", "required": true}},
//!     "rules": [{"type": "endpoint", "conditions": [],
//!                "endpoint": {"url": "https://example.{Region}.amazonaws.com"}}]}"#;
//! let program = Program::decode(rules).map_err(|e| e.to_string())?;
//! let mut params = Params::new(&program);
//! let region = program.param("Region").ok_or("no Region")?.slot();
//! params.set(region, EvalValue::str("us-west-2"));
//! let partitions = Partitions::empty();
//! let outcome = program.evaluate(&params, &Library { partitions: &partitions });
//! let Ok(Outcome::Endpoint(endpoint)) = outcome else { return Err("no endpoint".into()) };
//! assert_eq!(endpoint.url, "https://example.us-west-2.amazonaws.com");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
#![forbid(unsafe_code)]

mod decode;
mod error;
mod eval;
mod ir;
mod stdlib;
mod template;

pub use decode::decode_rule_set;
pub use error::{DecodeError, Document, EvalError};
pub use ir::{EvalValue, ParamType, ParameterDef, RuleSet, Slot};

/// A decoded rule set, ready to evaluate. Only the tree form exists; a
/// BDD form would sit beside it and share the conditions, expressions,
/// [`Params`] and [`Outcome`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Program {
    Tree(RuleSet),
}

impl Program {
    /// Decodes an `endpoint-rule-set-1.json`.
    pub fn decode(json: &[u8]) -> Result<Self, DecodeError> {
        decode_rule_set(json).map(Self::Tree)
    }
    /// The rule set's parameters, in document order.
    pub fn params(&self) -> &[ParameterDef] {
        match self {
            Self::Tree(rules) => &rules.params,
        }
    }
    /// The parameter named `name`.
    pub fn param(&self, name: &str) -> Option<&ParameterDef> {
        self.params().iter().find(|param| param.name() == name)
    }
    /// Evaluates the program for `args`.
    pub fn evaluate(&self, args: &Params, _lib: &Library<'_>) -> Result<Outcome, EvalError> {
        match self {
            Self::Tree(rules) => eval::evaluate(rules, args),
        }
    }
}

/// A call's parameter values, by [`ParameterDef::slot`]. The evaluator
/// applies defaults; like the CRT engine it does not check a value
/// against its parameter's type, which a function then refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Params(Vec<Option<EvalValue>>);

impl Params {
    /// No values, for `program`'s parameters.
    pub fn new(program: &Program) -> Self {
        Self(vec![None; program.params().len()])
    }
    /// Sets the parameter at `slot`; a slot that is not a parameter's is
    /// ignored.
    pub fn set(&mut self, slot: Slot, value: EvalValue) {
        if let Some(entry) = self.0.get_mut(slot.index()) {
            *entry = Some(value);
        }
    }
    pub(crate) fn get(&self, slot: Slot) -> Option<&EvalValue> {
        self.0.get(slot.index()).and_then(Option::as_ref)
    }
}

/// What a rule set resolves to: an endpoint, or a rule's error message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Endpoint(Endpoint),
    Error(String),
}

/// A resolved endpoint. `properties` is a JSON object in document order
/// (empty when the rule has none); `headers` keep the rule's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub url: String,
    pub properties: serde_json::Value,
    pub headers: Vec<(String, Vec<String>)>,
}

/// What the AWS functions read.
#[derive(Debug, Clone, Copy)]
pub struct Library<'a> {
    pub partitions: &'a Partitions,
}

/// The partitions `aws.partition` matches a region against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partitions {
    _decoded_later: (),
}

impl Partitions {
    /// No partitions.
    pub fn empty() -> Self {
        Self { _decoded_later: () }
    }
}

#[cfg(test)]
mod tests;
