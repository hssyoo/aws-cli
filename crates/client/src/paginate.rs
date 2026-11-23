//! botocore's paginator (`botocore/paginate.py`) over [`Client::call`]:
//! one operation's pages, as `Paginator.paginate` returns them in a
//! `PageIterator`, and the full result the CLI's JSON output builds from
//! them (`PageIterator.build_full_result`).
//!
//! The paginator configuration's expressions are limited to member names
//! and dotted paths of them; an entry with any other JMESPath expression is
//! [`Refused`], so admission can leave its operation to Python.
use crate::call::Client;
use crate::error::CallError;
use model::value::{Map, Value};

/// One operation's entry in `paginators-1.json`, as `Paginator` reads it:
/// every token and key a list, as `_get_input_tokens` and kin make them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub input_token: Vec<String>,
    pub output_token: Vec<String>,
    pub more_results: Option<String>,
    pub result_key: Vec<String>,
    pub non_aggregate_keys: Vec<String>,
    pub limit_key: Option<String>,
}

/// Why a paginator entry is not one the native paginator runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// A JMESPath expression beyond a member name or a dotted path.
    Expression(String),
    /// An entry botocore itself could not read.
    Malformed(String),
}

impl Config {
    /// The entry `entry`, a value of the file's `pagination` map
    /// ([`Value::from_serde`] of it).
    ///
    /// # Errors
    /// See [`Refused`].
    pub fn from_value(entry: &Value) -> Result<Self, Refused> {
        let _ = entry;
        Err(Refused::Malformed(
            "paginator entries are not read yet".into(),
        ))
    }
}

/// `PaginationConfig`: what `--max-items`, `--page-size` and
/// `--starting-token` set (`PageArgument.add_to_params`), each as the
/// argument parsed it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaginationConfig {
    pub max_items: Option<Value>,
    pub page_size: Option<Value>,
    pub starting_token: Option<String>,
}

/// `PageIterator`: the pages of `operation` called with `input`, each
/// the parsed output of one call.
#[derive(Debug)]
pub struct PageIterator<'c> {
    client: &'c Client,
    operation: String,
    input: Value,
    config: Config,
    pagination: PaginationConfig,
    non_aggregate_part: Map,
}

impl<'c> PageIterator<'c> {
    /// `Paginator.paginate(**input, PaginationConfig=pagination)`.
    pub fn new(
        client: &'c Client,
        operation: &str,
        input: Value,
        config: Config,
        pagination: PaginationConfig,
    ) -> Self {
        Self {
            client,
            operation: operation.to_owned(),
            input,
            config,
            pagination,
            non_aggregate_part: Map::new(),
        }
    }

    /// `result_keys`: the configured result keys, as expressions.
    pub fn result_keys(&self) -> &[String] {
        &self.config.result_key
    }

    /// `non_aggregate_part`: the non-aggregate keys' values the pages so far
    /// set.
    pub fn non_aggregate_part(&self) -> &Map {
        &self.non_aggregate_part
    }

    /// `resume_token`: the encoded token to resume from, once iteration
    /// stopped short of the last page.
    pub fn resume_token(&self) -> Option<&str> {
        None
    }

    /// `build_full_result`: every page's result keys merged, the
    /// non-aggregate part, and `NextToken` when truncated.
    ///
    /// # Errors
    /// The first page's call failure.
    pub fn build_full_result(&mut self) -> Result<Value, CallError> {
        let _ = (&self.client, &self.operation, &self.input, &self.pagination);
        Err(CallError::UnknownOperation(format!(
            "{}: pagination is not implemented yet",
            self.operation
        )))
    }
}

impl Iterator for PageIterator<'_> {
    type Item = Result<Value, CallError>;

    /// `PageIterator.__iter__`'s next page.
    fn next(&mut self) -> Option<Self::Item> {
        None
    }
}

#[cfg(test)]
mod tests;
