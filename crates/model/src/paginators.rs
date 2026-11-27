//! A service's paginators file (`paginators-1.json`), as botocore's
//! `PaginatorModel` holds it: read once, each operation's entry parsed once
//! into what its two readers take from it. `unify_paging_params` reads the
//! input tokens and the limit key to edit the operation's argument table
//! whatever else the entry holds; `Paginator` reads the whole entry to
//! page, and the native paginator runs only an entry whose expressions it
//! evaluates ([`Expression`]), and whose result and non-aggregate keys,
//! which it also writes, are member names and dotted paths of them.
use crate::pyrepr::py_repr;
use crate::source::{Kind, LoadError, Source, load_service_model};
use crate::value::{Map, Value};

mod expression;

pub use expression::{Assign, Expression, Lookup, NotADict, PAGINATORS};

/// Every operation's entry, by its model name, in the file's order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paginators {
    entries: Vec<(String, Entry)>,
}

/// One operation's entry in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// `get_input_tokens(paginator_config)`: the input tokens that are
    /// strings, as `unify_paging_params` reads them to hide their
    /// arguments.
    pub input_token: Vec<String>,
    /// `paginator_config.get('limit_key')`, when a string.
    pub limit_key: Option<String>,
    /// The entry as the paginator runs it, or why the native one does not.
    pub config: Result<Config, Refused>,
}

/// One operation's entry as `Paginator` reads it: every token and key a
/// list, as `_get_input_tokens` and kin make them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub input_token: Vec<String>,
    pub output_token: Vec<Expression>,
    pub more_results: Option<Expression>,
    pub result_key: Vec<Expression>,
    pub non_aggregate_keys: Vec<Expression>,
    pub limit_key: Option<String>,
}

/// Why a paginator entry is not one the native paginator runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// A JMESPath expression the paginator does not evaluate, or a result
    /// or non-aggregate key beyond a member name or a dotted path.
    Expression(String),
    /// An entry botocore itself could not read.
    Malformed(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Expression(expression) => write!(f, "a paginator expression {expression}"),
            Self::Malformed(why) => write!(f, "a paginator entry: {why}"),
        }
    }
}

impl Paginators {
    /// `Loader.load_service_model(service, 'paginators-1')['pagination']`;
    /// none for a service without the file, as `client.can_paginate`
    /// reads `DataNotFoundError` (of which an unknown service is one).
    ///
    /// # Errors
    /// The file could not be read.
    pub fn load(source: &impl Source, service: &str) -> Result<Self, LoadError> {
        match load_service_model(source, service, Kind::Paginators) {
            Ok(document) => Ok(Self::from_json(&document.json)),
            Err(LoadError::DataNotFound(_) | LoadError::UnknownService(_)) => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    /// The entries of a paginators document's `pagination` object.
    pub fn from_json(document: &serde_json::Value) -> Self {
        let entries = document
            .get("pagination")
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flatten()
            .map(|(operation, entry)| {
                (
                    operation.clone(),
                    Entry::from_value(&Value::from_serde(entry)),
                )
            })
            .collect();
        Self { entries }
    }

    /// The entry of the model operation `operation`
    /// (`PaginatorModel.get_paginator`); `None` when it has none
    /// (`client.can_paginate`).
    pub fn get(&self, operation: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|(name, _)| name == operation)
            .map(|(_, entry)| entry)
    }

    /// Every entry, by operation, in the file's order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Entry)> {
        self.entries
            .iter()
            .map(|(name, entry)| (name.as_str(), entry))
    }
}

impl Entry {
    /// `entry`, a value of the file's `pagination` object.
    pub fn from_value(entry: &Value) -> Self {
        let field = |key: &str| match entry {
            Value::Map(map) => map.get(key),
            _ => None,
        };
        let input_token = field("input_token")
            .map(listed)
            .unwrap_or_default()
            .iter()
            .filter_map(|token| match token {
                Value::String(token) => Some(token.clone()),
                _ => None,
            })
            .collect();
        let limit_key = match field("limit_key") {
            Some(Value::String(key)) => Some(key.clone()),
            _ => None,
        };
        Self {
            input_token,
            limit_key,
            config: Config::from_value(entry),
        }
    }
}

impl Config {
    /// The entry `entry`, a value of the file's `pagination` object, read
    /// as `Paginator.__init__` reads it: `output_token`, `input_token` and
    /// `result_key` a string or a list of them, `non_aggregate_keys` a
    /// list, `more_results` and `limit_key` a string. Each expression
    /// `jmespath.compile`s must be one [`Expression::parse`] gives; a
    /// result key and a non-aggregate key, which `set_value_from_jmespath`
    /// also writes, must be a name or a dotted path of names.
    /// `input_token` and `limit_key` are parameter names, which botocore
    /// sends as they are.
    ///
    /// # Errors
    /// See [`Refused`]. An entry without `result_key`, or with an empty
    /// one, is malformed: iterating its pages fails in botocore.
    pub fn from_value(entry: &Value) -> Result<Self, Refused> {
        let Value::Map(entry) = entry else {
            return Err(malformed("the entry is not an object"));
        };
        let result_key = paths(required(entry, "result_key")?)?;
        if result_key.is_empty() {
            return Err(malformed("result_key is empty"));
        }
        let non_aggregate_keys = match entry.get("non_aggregate_keys") {
            None => Vec::new(),
            Some(Value::List(keys)) => keys.iter().map(path).collect::<Result<_, _>>()?,
            Some(_) => return Err(malformed("non_aggregate_keys is not a list")),
        };
        Ok(Self {
            input_token: strings(required(entry, "input_token")?)?,
            output_token: expressions(required(entry, "output_token")?)?,
            more_results: optional(entry, "more_results")
                .map(expression)
                .transpose()?,
            result_key,
            non_aggregate_keys,
            limit_key: optional(entry, "limit_key").map(name).transpose()?,
        })
    }
}

fn malformed(why: &str) -> Refused {
    Refused::Malformed(why.to_owned())
}

/// `config[key]`, which botocore requires.
fn required<'v>(entry: &'v Map, key: &str) -> Result<&'v Value, Refused> {
    match entry.get(key) {
        None | Some(Value::Null) => Err(Refused::Malformed(format!("{key} is missing"))),
        Some(value) => Ok(value),
    }
}

/// `config.get(key)`.
fn optional<'v>(entry: &'v Map, key: &str) -> Option<&'v Value> {
    entry.get(key).filter(|value| !matches!(value, Value::Null))
}

/// A string, or a list of them, as a list.
fn listed(value: &Value) -> &[Value] {
    match value {
        Value::List(items) => items,
        single => std::slice::from_ref(single),
    }
}

fn strings(value: &Value) -> Result<Vec<String>, Refused> {
    listed(value).iter().map(name).collect()
}

fn expressions(value: &Value) -> Result<Vec<Expression>, Refused> {
    listed(value).iter().map(expression).collect()
}

fn paths(value: &Value) -> Result<Vec<Expression>, Refused> {
    listed(value).iter().map(path).collect()
}

/// A parameter name.
fn name(value: &Value) -> Result<String, Refused> {
    match value {
        Value::String(text) => Ok(text.clone()),
        other => Err(Refused::Malformed(format!(
            "{} is not a string",
            py_repr(other)
        ))),
    }
}

/// An expression this paginator runs.
fn expression(value: &Value) -> Result<Expression, Refused> {
    let text = name(value)?;
    Expression::parse(&text).ok_or(Refused::Expression(text))
}

/// An expression this paginator reads and writes: a name or a dotted path.
fn path(value: &Value) -> Result<Expression, Refused> {
    let found = expression(value)?;
    if found.is_path() {
        Ok(found)
    } else {
        Err(Refused::Expression(found.as_str().to_owned()))
    }
}

#[cfg(test)]
mod tests;
