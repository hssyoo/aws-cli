//! botocore's paginator (`botocore/paginate.py`) over [`Client::call`]:
//! one operation's pages, as `Paginator.paginate` returns them in a
//! `PageIterator`, and the full result the CLI's JSON output builds from
//! them (`PageIterator.build_full_result`).
//!
//! The paginator configuration is the service's typed paginators file
//! ([`model::paginators`]), whose expressions are member names and dotted
//! paths of them; an entry with any other JMESPath expression is refused
//! there, so admission can leave its operation to Python.
//!
//! Every Python operation the paginator applies to a page (`len`, slices,
//! `+`, `set_value_from_jmespath`) fails here as it fails there, with the
//! exception's text, as a [`PaginationError`]. Pages are worked on as
//! Python's shared objects ([`object::Object`]), so what the non-aggregate
//! part and the full result share with the first page changes with it as
//! it does there; a page is returned as its value then.
use crate::call::{Client, Prepared};
use crate::error::{CallError, PaginationError};
use cli_core::client_config::user_agent::{Feature, Features};
use model::paginators::{Config, Expression};
use model::pyrepr::{py_repr, py_str};
use model::schema::ShapeType;
use model::value::{Key, Map, Value};
use std::borrow::Borrow;

mod object;
mod path;
mod py;
mod token;

use object::Object;
/// The token codec, for the fuzz target over it.
#[cfg(any(test, feature = "testing"))]
pub(crate) use token::{TRUNCATE_AMOUNT, parse_starting_token, resume_token};

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
/// the parsed output of one call, through `client`: a [`Client`] it owns
/// or borrows.
///
/// It splits its work as botocore does: [`Self::new`] checks `MaxItems`
/// and `PageSize` as `paginate` does (`_extract_paging_params`), and what
/// `__iter__` does before its first request happens as the first page is
/// asked for, or earlier through [`Self::prepare`]; what follows a page's
/// `yield` happens as the next one is. Once a page's call or the paginator
/// fails, iteration ends with that error; the pages already returned stay
/// returned.
#[derive(Debug)]
pub struct PageIterator<C> {
    client: C,
    operation: String,
    /// `paginate`'s keyword arguments, as given.
    input: Value,
    config: Config,
    /// `MaxItems`, as `int()` read it.
    max_items: Option<i128>,
    /// `PageSize`, as the limit key's member takes it.
    page_size: Option<Value>,
    starting_token: Option<String>,
    /// `_non_aggregate_part`, which shares its objects with the first page.
    non_aggregate: Object,
    /// [`Self::non_aggregate`]'s value once the first page is returned,
    /// and again once the full result is built.
    non_aggregate_part: Map,
    resume_token: Option<String>,
    state: State,
}

/// Where `__iter__`'s generator is.
#[derive(Debug)]
enum State {
    /// `paginate` returned; nothing has run yet.
    Start,
    /// `__iter__` ran up to its first request, which is prepared.
    Prepared(Box<Progress>, Box<Prepared>),
    /// A page was yielded whole; the loop goes on with what it read of it.
    Yielded(Box<Progress>, Read),
    /// The loop ended.
    Done,
}

/// `__iter__`'s locals across its loop.
#[derive(Debug)]
struct Progress {
    /// The starting token's `boto_truncate_amount`, until the first page
    /// takes it.
    starting_index: Option<Value>,
    first_request: bool,
    /// The token of the request in flight.
    next_token: Map,
    previous_next_token: Option<Map>,
    total_items: i128,
    /// `current_kwargs`: the input of the request in flight.
    kwargs: Value,
}

/// What the loop reads of a page it yielded whole.
#[derive(Debug)]
struct Read {
    /// The primary result key's `len`.
    items: i128,
    /// `_get_next_token` of the page.
    next_token: Map,
}

impl<C: Borrow<Client>> PageIterator<C> {
    /// `Paginator.paginate(**input, PaginationConfig=pagination)`, which
    /// reads `MaxItems` and `PageSize` as it is called.
    ///
    /// # Errors
    /// A `MaxItems` or `PageSize` `_extract_paging_params` refuses.
    pub fn new(
        client: C,
        operation: &str,
        input: Value,
        config: Config,
        pagination: PaginationConfig,
    ) -> Result<Self, CallError> {
        let max_items = match given(pagination.max_items.as_ref()) {
            Some(value) => Some(py::int_number(&py::int(value)?)),
            None => None,
        };
        let page_size = page_size(client.borrow(), operation, &config, &pagination)?;
        Ok(Self {
            client,
            operation: operation.to_owned(),
            input,
            config,
            max_items,
            page_size,
            starting_token: pagination.starting_token,
            non_aggregate: Object::dict(),
            non_aggregate_part: Map::new(),
            resume_token: None,
            state: State::Start,
        })
    }

    /// `__iter__` up to its first request, and that request prepared as
    /// [`Client::prepare`] prepares a call, so a caller learns before
    /// anything is sent what the first page would fail with or leave to
    /// Python; the first page then sends it. Once a page was asked for, it
    /// does nothing.
    ///
    /// # Errors
    /// A bad starting token, anything `prepare` raises, and
    /// [`CallError::Unsupported`] for a starting token only Python reads.
    pub fn prepare(&mut self) -> Result<(), CallError> {
        if !matches!(self.state, State::Start) {
            return Ok(());
        }
        let progress = self.prologue()?;
        let prepared = self.client.borrow().prepare_with_features(
            &self.operation,
            progress.kwargs.clone(),
            paginator_feature(),
        )?;
        self.state = State::Prepared(Box::new(progress), Box::new(prepared));
        Ok(())
    }

    /// `result_keys`: the configured result keys, as expressions.
    pub fn result_keys(&self) -> &[Expression] {
        &self.config.result_key
    }

    /// `non_aggregate_part`: the non-aggregate keys' values the first page
    /// set (`_record_non_aggregate_key_values`), each `None` it lacked, as
    /// they are once that page is returned: botocore keeps the page's own
    /// objects, so a result key within one is the page's, truncated.
    pub fn non_aggregate_part(&self) -> &Map {
        &self.non_aggregate_part
    }

    /// `resume_token`: the encoded token to resume from, once iteration
    /// stopped short of the last page.
    pub fn resume_token(&self) -> Option<&str> {
        self.resume_token.as_deref()
    }

    /// `build_full_result`: every page's result keys merged, the
    /// non-aggregate part, and `NextToken` when truncated. A list extends
    /// what earlier pages gave, an `int`, `float` or `str` adds to it, and
    /// any other value leaves the first page's; a page lacking a key adds
    /// nothing. It takes the pages not yet taken.
    ///
    /// As in botocore, the first list of a result key is the page's own,
    /// extended in place, so a non-aggregate key holding it (devops-guru
    /// `GetResourceCollection`'s `ResourceCollection`) merges back the
    /// joined list.
    ///
    /// # Errors
    /// The first failure of a page's call or of the paginator.
    pub fn build_full_result(&mut self) -> Result<Value, CallError> {
        let complete = Object::dict();
        while let Some(page) = self.next_object() {
            let page = page?;
            for key in &self.config.result_key {
                if let Some(value) = key.search(page.clone()) {
                    merge(&complete, key, &value).map_err(other)?;
                }
            }
        }
        path::merge_dicts(&complete, &self.non_aggregate).map_err(other)?;
        self.non_aggregate_part = self.non_aggregate.map();
        if let Some(token) = &self.resume_token {
            let token = Object::Value(Value::String(token.clone()));
            complete.insert("NextToken", token).map_err(other)?;
        }
        Ok(complete.value())
    }
}

/// One result key's `value` from a page, merged into `complete`: the first
/// set as it is, a later list added to it, a later scalar added to it.
fn merge(complete: &Object, key: &Expression, value: &Object) -> Result<(), String> {
    let Some(existing) = key.search(complete.clone()) else {
        return key.set(complete.clone(), value.clone());
    };
    match value {
        Object::List(_) => existing.extend(value),
        Object::Value(
            scalar @ (Value::Integer(_)
            | Value::BigInteger(_)
            | Value::Float(_)
            | Value::Bool(_)
            | Value::String(_)
            | Value::PyString(_)),
        ) => {
            let sum = py::add(&existing.value(), scalar)?;
            key.set(complete.clone(), Object::Value(sum))
        }
        _ => Ok(()),
    }
}

/// An exception of Python's own, by its text.
fn other(text: String) -> CallError {
    CallError::Pagination(PaginationError::Other(text))
}

/// A `PaginationConfig` value, `None` being no value.
fn given(value: Option<&Value>) -> Option<&Value> {
    value.filter(|value| !matches!(value, Value::Null))
}

/// The feature id `PageIterator._make_request` registers for each page's
/// call (`with_current_context(partial(register_feature_id, ...))`).
fn paginator_feature() -> Features {
    [Feature::Paginator].into_iter().collect()
}

/// `_extract_paging_params`' page size: refused without a limit key, and
/// as the limit key's member takes it, a string `str()`, anything else
/// `int()`.
fn page_size(
    client: &Client,
    operation: &str,
    config: &Config,
    pagination: &PaginationConfig,
) -> Result<Option<Value>, CallError> {
    let Some(size) = given(pagination.page_size.as_ref()) else {
        return Ok(None);
    };
    let Some(limit_key) = &config.limit_key else {
        return Err(PaginationError::Pagination(
            "PageSize parameter is not supported for the pagination interface for this \
             operation."
                .into(),
        )
        .into());
    };
    let found = (client.model().schema().service())
        .operation(operation)
        .ok_or_else(|| CallError::UnknownOperation(operation.to_owned()))?;
    let input = found
        .input()
        .ok_or_else(|| other("'NoneType' object has no attribute 'members'".into()))?;
    let member = input
        .member(limit_key)
        .ok_or_else(|| other("'NoneType' object has no attribute 'type_name'".into()))?;
    if member.shape_type() != ShapeType::String {
        return Ok(Some(py::int(size)?));
    }
    Ok(Some(match size {
        Value::String(_) | Value::PyString(_) => size.clone(),
        size => Value::String(py_str(size)),
    }))
}

impl<C: Borrow<Client>> Iterator for PageIterator<C> {
    type Item = Result<Value, CallError>;

    /// `PageIterator.__iter__`'s next page.
    fn next(&mut self) -> Option<Self::Item> {
        self.next_object().map(|page| page.map(|page| page.value()))
    }
}

impl<C: Borrow<Client>> PageIterator<C> {
    /// The next page, the object itself.
    fn next_object(&mut self) -> Option<Result<Object, CallError>> {
        let progress = match std::mem::replace(&mut self.state, State::Done) {
            State::Start => self.prologue().map(Some),
            State::Prepared(progress, prepared) => {
                return Some(self.send(*progress, *prepared));
            }
            State::Yielded(progress, read) => self.resume(*progress, read),
            State::Done => Ok(None),
        };
        match progress {
            Ok(Some(progress)) => Some(self.request(progress)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        }
    }

    /// `__iter__` up to its loop: the starting token read and, with the
    /// page size, put into the input (`_inject_starting_params`).
    fn prologue(&self) -> Result<Progress, CallError> {
        let starting = match &self.starting_token {
            Some(raw) => Some(token::parse_starting_token(raw, &self.config.input_token)?),
            None => None,
        };
        // `self.result_keys[0]`.
        if self.config.result_key.is_empty() {
            return Err(other("list index out of range".into()));
        }
        let next_token = match &starting {
            Some(starting) => starting.token.clone(),
            None => self
                .config
                .input_token
                .iter()
                .map(|key| (key.clone(), Value::Null))
                .collect(),
        };
        let mut kwargs = self.input.clone();
        if let Some(starting) = &starting {
            inject(&mut kwargs, &starting.token);
        }
        if let (Some(size), Some(limit_key)) = (&self.page_size, &self.config.limit_key) {
            insert(&mut kwargs, &Key::from(limit_key.as_str()), size.clone());
        }
        Ok(Progress {
            starting_index: starting.map(|starting| starting.index),
            first_request: true,
            next_token,
            previous_next_token: None,
            total_items: 0,
            kwargs,
        })
    }

    /// `_make_request`, with `PAGINATOR` registered, then the loop's work
    /// on the page up to its `yield`.
    fn request(&mut self, progress: Progress) -> Result<Object, CallError> {
        let output = self.client.borrow().call_with_features(
            &self.operation,
            progress.kwargs.clone(),
            paginator_feature(),
        )?;
        self.page(progress, Object::new(output.value))
    }

    /// The first request, which [`Self::prepare`] prepared, sent, then the
    /// loop's work on its page.
    fn send(&mut self, progress: Progress, prepared: Prepared) -> Result<Object, CallError> {
        let output = self.client.borrow().send(prepared)?;
        self.page(progress, Object::new(output.value))
    }

    /// The loop's body from the response to the `yield`: the first page's
    /// starting offset and non-aggregate keys, then the page truncated to
    /// `MaxItems`, which ends the loop, or read for what follows.
    fn page(&mut self, mut progress: Progress, page: Object) -> Result<Object, CallError> {
        let mut starting_truncation = Value::Integer(0);
        let first = progress.first_request;
        if first {
            if let Some(index) = progress.starting_index.take() {
                starting_truncation = self.handle_first_request(&page, index)?;
            }
            progress.first_request = false;
            self.record_non_aggregate_key_values(&page)?;
        }
        let items = self
            .primary()
            .and_then(|primary| primary.search(page.clone()))
            .map_or(Ok(0), |found| found.len())
            .map_err(other)?;
        let truncate_amount = self
            .max_items
            .map_or(0, |max| (progress.total_items + items).saturating_sub(max));
        if truncate_amount > 0 {
            let next_token = progress.next_token;
            self.truncate_response(&page, truncate_amount, &starting_truncation, next_token)?;
        } else {
            let next_token = self.get_next_token(&page);
            self.state = State::Yielded(Box::new(progress), Read { items, next_token });
        }
        // Only the first page's objects are in the non-aggregate part, and
        // only `build_full_result` changes them after its truncation.
        if first {
            self.non_aggregate_part = self.non_aggregate.map();
        }
        Ok(page)
    }

    /// The rest of the loop after a whole page's `yield`: stop on no next
    /// token, or on `MaxItems` at a page boundary, which resumes from that
    /// token; fail on the token the last request sent; else send it.
    fn resume(
        &mut self,
        mut progress: Progress,
        read: Read,
    ) -> Result<Option<Progress>, CallError> {
        progress.total_items += read.items;
        let next_token = read.next_token;
        if next_token
            .iter()
            .all(|(_, token)| matches!(token, Value::Null))
        {
            return Ok(None);
        }
        if self.max_items == Some(progress.total_items) {
            self.resume_token = Some(token::resume_token(&next_token, &self.config.input_token)?);
            return Ok(None);
        }
        let repeated = progress
            .previous_next_token
            .as_ref()
            .is_some_and(|previous| py::map_eq(previous, &next_token));
        if repeated {
            let token = py_repr(&Value::Map(next_token));
            return Err(PaginationError::Pagination(format!(
                "The same next token was received twice: {token}"
            ))
            .into());
        }
        inject(&mut progress.kwargs, &next_token);
        progress.previous_next_token = Some(next_token.clone());
        progress.next_token = next_token;
        Ok(Some(progress))
    }

    /// The primary result key, `result_keys[0]`, which [`Self::start`]
    /// found.
    fn primary(&self) -> Option<&Expression> {
        self.config.result_key.first()
    }
}

impl<C: Borrow<Client>> PageIterator<C> {
    /// `_handle_first_request`: the primary result key, a list or a str,
    /// from the starting token's offset on (anything else `None`), and
    /// the others emptied, as the page they were taken whole from: a list
    /// `[]`, a str `''`, a number kept, anything else `None`.
    fn handle_first_request(&self, page: &Object, index: Value) -> Result<Value, CallError> {
        let Some((primary, secondary)) = self.config.result_key.split_first() else {
            return Ok(index);
        };
        let data = match primary.search(page.clone()) {
            Some(
                all @ (Object::List(_) | Object::Value(Value::String(_) | Value::PyString(_))),
            ) => all.slice_from(&index).map_err(other)?,
            _ => Object::NONE,
        };
        primary.set(page.clone(), data).map_err(other)?;
        for key in secondary {
            let empty = match key.search(page.clone()) {
                Some(Object::List(_)) => Object::list(Vec::new()),
                Some(Object::Value(Value::String(_) | Value::PyString(_))) => {
                    Object::Value(Value::String(String::new()))
                }
                Some(
                    number @ Object::Value(
                        Value::Integer(_) | Value::BigInteger(_) | Value::Float(_) | Value::Bool(_),
                    ),
                ) => number,
                _ => Object::NONE,
            };
            key.set(page.clone(), empty).map_err(other)?;
        }
        Ok(index)
    }

    /// `_record_non_aggregate_key_values`: each key's object in the page,
    /// itself, not a copy.
    fn record_non_aggregate_key_values(&mut self, page: &Object) -> Result<(), CallError> {
        let part = Object::dict();
        for key in &self.config.non_aggregate_keys {
            let value = key.search(page.clone()).unwrap_or(Object::NONE);
            key.set(part.clone(), value).map_err(other)?;
        }
        self.non_aggregate = part;
        Ok(())
    }

    /// `_truncate_response`: the primary result key cut to what `MaxItems`
    /// leaves, set in the page's own dict, and the resume token the
    /// request's token with the offset into the page, counted from the
    /// page's start.
    fn truncate_response(
        &mut self,
        page: &Object,
        truncate_amount: i128,
        starting_truncation: &Value,
        mut next_token: Map,
    ) -> Result<(), CallError> {
        // A page is truncated only once its primary result key had items.
        let Some(primary) = self.primary().cloned() else {
            return Ok(());
        };
        let original = primary
            .search(page.clone())
            .unwrap_or_else(|| Object::list(Vec::new()));
        let amount_to_keep = original.len().map_err(other)? - truncate_amount;
        let truncated = original.slice_to(amount_to_keep).map_err(other)?;
        primary.set(page.clone(), truncated).map_err(other)?;
        let amount = py::int_add(amount_to_keep, starting_truncation).map_err(other)?;
        next_token.insert(token::TRUNCATE_AMOUNT.into(), amount);
        self.resume_token = Some(token::resume_token(&next_token, &self.config.input_token)?);
        Ok(())
    }

    /// `_get_next_token`: nothing once `more_results` is false, else each
    /// output token under its input token's name, a false one `None`.
    fn get_next_token(&self, page: &Object) -> Map {
        if let Some(more) = &self.config.more_results
            && !more.search(page.clone()).is_some_and(|more| more.truthy())
        {
            return Map::new();
        }
        let config = &self.config;
        config
            .output_token
            .iter()
            .zip(&config.input_token)
            .map(|(output, input)| {
                let token = output.search(page.clone()).filter(Object::truthy);
                (
                    input.clone(),
                    token.map_or(Value::Null, |token| token.value()),
                )
            })
            .collect()
    }
}

/// `_inject_token_into_kwargs`: each token sent, and one that is `None` or
/// `'None'` taken out.
fn inject(input: &mut Value, token: &Map) {
    for (name, value) in token.iter() {
        let unset = match value {
            Value::Null => true,
            Value::String(text) => text == "None",
            // A `str` with a lone surrogate is not `'None'`.
            Value::PyString(_) => false,
            _ => false,
        };
        if !unset {
            insert(input, name, value.clone());
        } else if let Value::Map(map) = input {
            map.remove_key(name);
        }
    }
}

/// `op_kwargs[name] = value`: a starting token's name as the token gave it.
fn insert(input: &mut Value, name: &Key, value: Value) {
    match input {
        Value::Map(map) => {
            map.insert_key(name.clone(), value);
        }
        other => *other = Value::Map(Map::from_iter([(name.clone(), value)])),
    }
}

#[cfg(test)]
mod tests;
