//! The request a completed parse binds, as `ServiceOperation.
//! _build_call_parameters` builds it: one argument at a time in the table's
//! order, each supplied value under its member's name, as `add_to_params`
//! stores it (`CLIArgument`, `BooleanArgument`). An unset value adds
//! nothing. The client keeps this order on the wire.
//!
//! A paginated operation's paging handlers run around it, as
//! `unify_paging_params` registers them: `check_should_enable_pagination`
//! before it, which turns paging off for a manual token and then raises
//! for a page option or puts back the arguments the page options shadowed;
//! `PageArgument.add_to_params` within it, which keeps the page options
//! under `PaginationConfig`; and `check_should_enable_pagination_call_
//! parameters` after it, which turns paging off for a token in the request.
//!
//! The unpack loop already ran (`awscli_argparse::complete`): every value
//! here is a string, an integer or a boolean member's, as the derived table
//! says (`command::models::Unpack`), because any other supplied argument
//! stopped it at its destination.
use awscli_argparse::ParseResult;
use awscli_argparse::namespace::{Namespace, Value as Parsed};
use cli_core::cli_error::{Category, CommandFailure, PythonReason};
use client::paginate::PaginationConfig;
use command::models::{DerivedArgument, Origin, Paging, Table, Unpack};
use model::value::{Map, Value};

/// What binding decided: the call, or what a paging handler raised first.
#[derive(Debug, Clone, PartialEq)]
pub enum Binding {
    /// The call to make.
    Call(Call),
    /// `ensure_paging_params_not_set` raised on `operation-args-parsed`,
    /// before the request was built.
    Raised(CommandFailure),
}

/// The request, and how `_make_client_call` sends it.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub input: Value,
    /// `PaginationConfig`, which `Paginator.paginate` takes from the
    /// request, when the operation has a paginator and paging stayed on;
    /// `None` for one call, whose request keeps any `PaginationConfig`.
    pub pagination: Option<PaginationConfig>,
    /// `PageArgument.add_to_params` warned that `--max-items` is not
    /// positive.
    pub warned: bool,
}

/// `PageArgument._emit_non_positive_max_items_warning`'s text.
pub const MAX_ITEMS_WARNING: &str =
    "warning: Non-positive values for --max-items may result in undefined behavior.\n";

/// The operation's call from the completed `parsed`, or why Python binds
/// it.
pub fn bind(table: &Table, parsed: &ParseResult<'_>) -> Result<Binding, PythonReason> {
    let paging = table.paging.as_ref();
    let raw = &parsed.args.namespace;
    if let Some(failure) = bool_conflict(table, raw) {
        return Ok(Binding::Raised(failure));
    }
    let mut paginate = paging.is_some() && paginate_global(parsed);
    if let Some(paging) = paging {
        paginate &= !manual_token(paging, raw);
        if !paginate && let Some(failure) = paging_params_set(paging, raw) {
            return Ok(Binding::Raised(failure));
        }
    }
    let shadowed = paging
        .filter(|_| !paginate)
        .map_or(&[][..], |paging| &paging.shadowed[..]);
    let mut input = Map::new();
    let mut warned = false;
    for argument in table
        .arguments
        .iter()
        .map(|argument| restored(argument, shadowed))
    {
        let dest = dest_of(argument);
        if let Origin::BoolValue { member, value, .. } = &argument.origin {
            if let Some(flag) = bool_value(*value, &parsed.values, &dest)? {
                let value = Map::from_iter([("Value".to_owned(), Value::Bool(flag))]);
                input.insert(member.to_string(), Value::Map(value));
            }
            continue;
        }
        let Some(value) = parsed.values.value(&dest).filter(|value| !value.is_null()) else {
            continue;
        };
        match (&argument.origin, argument.unpack) {
            (Origin::Member { member, .. }, unpack) if unpack != Unpack::Deferred => {
                input.insert(member.to_string(), scalar(value)?);
            }
            (Origin::StartingToken | Origin::PageSize | Origin::MaxItems, _) => {
                warned |= page_option(&mut input, &argument.origin, value)?;
            }
            _ => return Err(PythonReason::UnsupportedOption),
        }
    }
    paginate &= !paging.is_some_and(|paging| {
        input
            .keys()
            .any(|key| paging.tokens.iter().any(|token| *key == *token.model))
    });
    let pagination = paginate.then(|| pagination_config(input.remove(PAGINATION_CONFIG)));
    Ok(Binding::Call(Call {
        input: Value::Map(input),
        pagination,
        warned,
    }))
}

const PAGINATION_CONFIG: &str = "PaginationConfig";

/// What a `toplevelbool` argument's `add_to_params` stores under
/// `Value`: `PositiveBooleanArgument` given without a value, true (with
/// one, the unpack loop already deferred it); `NegativeBooleanParameter`
/// given, false; one left out (`_NOT_SPECIFIED`), nothing.
fn bool_value(
    positive: bool,
    values: &Namespace,
    dest: &str,
) -> Result<Option<bool>, PythonReason> {
    if !values.is_explicit(dest) {
        return Ok(None);
    }
    match (positive, values.value(dest)) {
        (true, Some(Parsed::Null)) => Ok(Some(true)),
        (false, Some(Parsed::Bool(true))) => Ok(Some(false)),
        _ => Err(PythonReason::UnsupportedValue),
    }
}

/// `validate_boolean_mutex_groups` on `operation-args-parsed.ec2.*`, whose
/// handlers run before the operation's own (the paging check's): the first
/// pair both of whose arguments were given raises.
fn bool_conflict(table: &Table, raw: &Namespace) -> Option<CommandFailure> {
    let given = |name: &str| {
        table
            .arguments
            .iter()
            .find(|argument| &*argument.name == name)
            .is_some_and(|argument| raw.is_explicit(&dest_of(argument)))
    };
    let (positive, negative) = table
        .bool_pairs
        .iter()
        .find(|(positive, negative)| given(positive) && given(negative))?;
    Some(CommandFailure {
        message: format!(
            "Cannot specify both the \"--{positive}\" option and the \"--{negative}\" option."
        )
        .into_bytes(),
        ..CommandFailure::new(Category::ParamValidation)
    })
}

/// `argument`, or the argument it shadowed, which `check_should_enable_
/// pagination` put back in the table when it turned paging off.
fn restored<'t>(
    argument: &'t DerivedArgument,
    shadowed: &'t [DerivedArgument],
) -> &'t DerivedArgument {
    shadowed
        .iter()
        .find(|held| held.name == argument.name)
        .unwrap_or(argument)
}

/// `parsed_globals.paginate`: false under `--no-paginate`.
fn paginate_global(parsed: &ParseResult<'_>) -> bool {
    parsed.globals.namespace.value("paginate") != Some(&Parsed::Bool(false))
}

/// `check_should_enable_pagination`'s test: an input token or the limit key
/// supplied by its own argument, unless its name is one of the
/// `normalized_paging_args`, `start_token` and `max_items`.
fn manual_token(paging: &Paging, raw: &Namespace) -> bool {
    paging.tokens.iter().any(|token| {
        let py_name = token.cli.replace('-', "_");
        !["start_token", "max_items"].contains(&py_name.as_str())
            && raw.value(&py_name).is_some_and(|value| !value.is_null())
    })
}

/// `ensure_paging_params_not_set`: the page options given a true value as
/// parsed, those a shadowed argument took back aside, raise.
fn paging_params_set(paging: &Paging, raw: &Namespace) -> Option<CommandFailure> {
    let shadowed: Vec<String> = paging
        .shadowed
        .iter()
        .map(|argument| argument.name.replace('-', "_"))
        .collect();
    let used: Vec<String> = ["starting_token", "page_size", "max_items"]
        .into_iter()
        .filter(|param| !shadowed.iter().any(|name| name == param))
        .filter(|param| raw.value(param).is_some_and(truthy))
        .map(|param| format!("--{}", param.replace('_', "-")))
        .collect();
    (!used.is_empty()).then(|| CommandFailure {
        message: format!(
            "Cannot specify --no-paginate along with pagination arguments: {}",
            used.join(", ")
        )
        .into_bytes(),
        ..CommandFailure::new(Category::ParamValidation)
    })
}

/// Python's truth value of a parsed value.
fn truthy(value: &Parsed) -> bool {
    match value {
        Parsed::Null => false,
        Parsed::Bool(flag) => *flag,
        Parsed::Number(digits) => digits != "0",
        Parsed::String(text) => !text.is_empty(),
        Parsed::OsString(text) => !text.is_empty(),
        Parsed::Bytes(bytes) => !bytes.is_empty(),
        Parsed::Array(items) => !items.is_empty(),
        Parsed::Object(pairs) => !pairs.is_empty(),
    }
}

/// `PageArgument.add_to_params`: the value under the option's key of the
/// request's `PaginationConfig`. True when the option is `--max-items` and
/// `int()` of its value is not positive, which warns; a value `int()`
/// rejects raises there, which only Python reports.
fn page_option(input: &mut Map, origin: &Origin, value: &Parsed) -> Result<bool, PythonReason> {
    let (key, value) = match (origin, value) {
        (Origin::StartingToken, Parsed::String(text)) => {
            ("StartingToken", Value::String(text.clone()))
        }
        (Origin::PageSize, _) => ("PageSize", scalar(value)?),
        (Origin::MaxItems, _) => ("MaxItems", scalar(value)?),
        _ => return Err(PythonReason::UnsupportedValue),
    };
    let warns = key == "MaxItems" && !positive(&value)?;
    match input.get_mut(PAGINATION_CONFIG) {
        Some(Value::Map(config)) => {
            config.insert(key.to_owned(), value);
        }
        _ => {
            let config = Map::from_iter([(key.to_owned(), value)]);
            input.insert(PAGINATION_CONFIG.to_owned(), Value::Map(config));
        }
    }
    Ok(warns)
}

/// Whether `int(value) > 0`: an integer as argparse's `int` gave it, or a
/// string-typed option's text as `int()` reads it.
fn positive(value: &Value) -> Result<bool, PythonReason> {
    match value {
        Value::Integer(number) => Ok(*number > 0),
        Value::BigInteger(digits) => Ok(!digits.starts_with('-')),
        Value::String(text) => {
            let converted = awscli_argparse::convert::convert(
                awscli_argparse::ValueType::Int,
                std::ffi::OsStr::new(text),
            );
            match converted {
                awscli_argparse::convert::Converted::Value(digits) => {
                    Ok(digits != "0" && !digits.starts_with('-'))
                }
                _ => Err(PythonReason::UnsupportedValue),
            }
        }
        _ => Err(PythonReason::UnsupportedValue),
    }
}

/// `Paginator._extract_paging_params`'s source: the page options as
/// `add_to_params` stored them.
fn pagination_config(config: Option<Value>) -> PaginationConfig {
    let Some(Value::Map(config)) = config else {
        return PaginationConfig::default();
    };
    PaginationConfig {
        max_items: config.get("MaxItems").cloned(),
        page_size: config.get("PageSize").cloned(),
        starting_token: match config.get("StartingToken") {
            Some(Value::String(token)) => Some(token.clone()),
            _ => None,
        },
    }
}

/// A string, an integer (any magnitude) or a boolean as the request holds
/// it.
fn scalar(value: &Parsed) -> Result<Value, PythonReason> {
    match value {
        Parsed::String(text) => Ok(Value::String(text.clone())),
        Parsed::Bool(flag) => Ok(Value::Bool(*flag)),
        Parsed::Number(digits) => Ok(digits.parse().map_or_else(
            |_| Value::BigInteger(digits.as_str().into()),
            Value::Integer,
        )),
        _ => Err(PythonReason::UnsupportedValue),
    }
}

/// Whether `validate_boolean_mutex_groups` raises, or a paging handler
/// raises or writes, before `_build_call_parameters`
/// unpacks the value that failed (`parsed.stopped_dest`), which is then
/// Python's to report: `ensure_paging_params_not_set`'s error on
/// `operation-args-parsed`, before every argument; or, with paging on,
/// `--max-items`' `add_to_params`, which warns when `int()` of its value is
/// not positive and raises when `int()` does, only when it comes before
/// the failing argument in the table.
pub fn precedes_unpack_failure(table: &Table, parsed: &ParseResult<'_>) -> bool {
    let raw = &parsed.args.namespace;
    if bool_conflict(table, raw).is_some() {
        return true;
    }
    let Some(paging) = table.paging.as_ref() else {
        return false;
    };
    let paginate = paginate_global(parsed) && !manual_token(paging, raw);
    if !paginate {
        return paging_params_set(paging, raw).is_some();
    }
    let Some(max_items) = raw.value("max_items").filter(|value| !value.is_null()) else {
        return false;
    };
    let acts = !scalar(max_items)
        .and_then(|value| positive(&value))
        .is_ok_and(|positive| positive);
    acts && parsed
        .stopped_dest
        .as_deref()
        .is_some_and(|failed| before(table, "max-items", failed))
}

/// Whether the argument named `name` comes before the one whose dest is
/// `dest` in the table, the order `_build_call_parameters` walks.
fn before(table: &Table, name: &str, dest: &str) -> bool {
    let position =
        |found: &dyn Fn(&DerivedArgument) -> bool| table.arguments.iter().position(found);
    let named = position(&|argument| &*argument.name == name);
    let failed = position(&|argument| dest_of(argument) == dest);
    matches!((named, failed), (Some(named), Some(failed)) if named < failed)
}

/// The argument's dest, as argparse names it.
fn dest_of(argument: &DerivedArgument) -> String {
    argument
        .dest
        .as_deref()
        .map_or_else(|| argument.name.replace('-', "_"), str::to_owned)
}

#[cfg(test)]
mod tests;
