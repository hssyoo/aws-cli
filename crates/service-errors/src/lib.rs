//! A model command's failures as the Python route renders them: what
//! `cli_core::cli_error` prints and exits with for a `client::CallError`,
//! the pager's failure, and an interrupt, in the enhanced and legacy
//! `cli_error_format` (structured formats route to Python).
//!
//! The driver calls [`call_failure`] or [`pager_failure`], then renders the
//! result with [`cli_core::cli_error::render_command_failure`] and exits
//! with [`cli_core::cli_error::exit_status`]. An interrupt is
//! [`interrupted`]: botocore's `KeyboardInterrupt` handler writes only to
//! stdout.
#![forbid(unsafe_code)]
use cli_core::botocore_error;
use cli_core::cli_error::{Category, CommandFailure, Details};
use client::error::{CallError, ServiceError};
use model::shapes::Model;
use model::value::{Map, Value};

/// `InterruptExceptionHandler`: a newline on stdout, flushed, and 130.
///
/// # Errors
/// Writing to `stdout` failed.
pub fn interrupted(stdout: &mut dyn std::io::Write) -> std::io::Result<u8> {
    stdout.write_all(b"\n")?;
    // The interpreter flushes stdout at exit; the binary's exit flushes
    // only what a failed write left.
    stdout.flush()?;
    Ok(cli_core::interrupt::EXIT_STATUS)
}

/// The failure botocore's exception for `error` is, as the CLI's handler
/// chain claims it (`construct_cli_error_handlers_chain`), for a call to
/// `model`'s service.
///
/// - `ParamValidationError`, a customization's among them:
///   `ParamValidationErrorsHandler`, 252.
/// - `NoRegionError`: `NoRegionErrorHandler`, 253, code `NoRegion`.
/// - `NoCredentialsError`: `NoCredentialsErrorHandler`, 253.
/// - `ClientError`: `ClientErrorHandler`, 254, with
///   `EnhancedErrorFormatter`'s details.
/// - `KeyboardInterrupt`: 130; see [`interrupted`].
/// - What the paginator raises (`PaginationError`, a bad starting token's
///   `ValueError`, and the rest): `GeneralExceptionHandler`, 255,
///   `str(exception)`.
/// - Anything else, the endpoint, credential provider, transport, parser,
///   serializer and hook failures among them: `GeneralExceptionHandler`,
///   255, `str(exception)`.
pub fn call_failure(error: &CallError, model: &Model) -> CommandFailure {
    match error {
        CallError::Validation(report) => CommandFailure {
            message: report.message().into_bytes(),
            ..CommandFailure::new(Category::ParamValidation)
        },
        // A customization's own parameter validation (`HookError::Validation`)
        // raises botocore's `ParamValidationError` with its text.
        CallError::Hook(client::hooks::HookError::Validation(text)) => CommandFailure {
            message: text.clone().into_bytes(),
            ..CommandFailure::new(Category::ParamValidation)
        },
        CallError::NoRegion => CommandFailure {
            code: "NoRegion".into(),
            message: format!(
                "{} You can also configure your region by running \"aws configure\".",
                botocore_error::NO_REGION
            )
            .into_bytes(),
            ..CommandFailure::new(Category::Configuration)
        },
        CallError::NoCredentials => CommandFailure {
            message: botocore_error::NO_CREDENTIALS.as_bytes().to_vec(),
            ..CommandFailure::new(Category::NoCredentials)
        },
        CallError::Service(service) => client_error(service, model),
        CallError::Interrupted => CommandFailure::new(Category::Interrupt),
        other => CommandFailure {
            message: other.to_string().into_bytes(),
            ..CommandFailure::new(Category::General)
        },
    }
}

/// `LazyPager`'s failure to start the pager: a missing program is
/// `PagerInitializationException` (`PagerErrorHandler`, 253, code
/// `Pager`), and an empty command's `IndexError` the general handler's
/// (255). Any other `OSError` is no failure: `format::pager::run`
/// reports the pager exited.
pub fn pager_failure(error: &format::pager::SpawnError) -> CommandFailure {
    if !error.not_found {
        return CommandFailure {
            message: error.text.clone().into_bytes(),
            ..CommandFailure::new(Category::General)
        };
    }
    CommandFailure {
        code: "Pager".into(),
        message: format!(
            "Unable to redirect output to pager. Received the following error when opening \
             pager:\n{}\n\nLearn more about configuring the output pager by running \"aws help \
             config-vars\".",
            error.text
        )
        .into_bytes(),
        ..CommandFailure::new(Category::Configuration)
    }
}

/// `ClientErrorHandler`: `str(ClientError)`, and in the enhanced format the
/// error's other modeled fields.
fn client_error(error: &ServiceError, model: &Model) -> CommandFailure {
    CommandFailure {
        code: error.code().unwrap_or("Unknown").to_owned(),
        operation: error.operation.clone(),
        message: error.message().unwrap_or("Unknown").as_bytes().to_vec(),
        retries_exhausted: error.max_attempts_reached.then_some(error.retry_attempts),
        details: details(&error_info(error, model)).map(|block| Box::new(Details(block))),
        http_status: i32::from(error.status),
        ..CommandFailure::new(Category::ClientError)
    }
}

/// `ClientErrorHandler._extract_error_response`: `parsed['Error']`, then
/// the response's other top-level keys it lacks, keeping only the fields
/// the error's shape models (`modeled_fields`, client.py: `Code`,
/// `Message`, and the members of `shape_for_error_code`), compared without
/// case.
fn error_info(error: &ServiceError, model: &Model) -> Map {
    let mut modeled: Vec<String> = vec!["code".into(), "message".into()];
    if let Some(shape) = error
        .code()
        .and_then(|code| model.shape_for_error_code(code))
    {
        modeled.extend(model.members(shape).iter().map(|m| m.name.to_lowercase()));
    }
    let is_modeled = |key: &str| modeled.iter().any(|name| *name == key.to_lowercase());
    let mut info = Map::new();
    for (key, value) in error.error.iter() {
        if is_modeled(key) {
            info.insert(key.to_owned(), value.clone());
        }
    }
    for (key, value) in error.members.iter() {
        if !error.error.contains_key(key) && is_modeled(key) {
            info.insert(key.to_owned(), value.clone());
        }
    }
    info
}

/// The most items a collection may hold to print inline
/// (`MAX_INLINE_ITEMS` is the first count that does not).
const MAX_INLINE_ITEMS: usize = 5;

/// `EnhancedErrorFormatter.format_error`: each field but `Code` and
/// `Message` (any case) as `key: value`, a small collection of simple
/// values inline, anything else `<complex value>` and, once, the hint;
/// `None` when there is no such field.
fn details(info: &Map) -> Option<Vec<u8>> {
    let mut lines = String::new();
    let mut complex = false;
    for (key, value) in info.iter() {
        if matches!(key.to_lowercase().as_str(), "code" | "message") {
            continue;
        }
        let shown = if is_simple(value) {
            model::pyrepr::py_str(value)
        } else if let Some(inline) = inline(value) {
            inline
        } else {
            complex = true;
            "<complex value>".to_owned()
        };
        lines.push_str(&format!("{key}: {shown}\n"));
    }
    if lines.is_empty() {
        return None;
    }
    let mut block = format!("\nAdditional error details:\n{lines}");
    if complex {
        block.push_str(
            "Use \"--cli-error-format json\" or another error format to see the full details.\n",
        );
    }
    Some(block.into_bytes())
}

/// `_is_simple_value`: `str`, `int`, `float`, `bool` or `None`. A blob is
/// `bytes` and a timestamp a `datetime`, neither simple.
fn is_simple(value: &Value) -> bool {
    matches!(
        value,
        Value::String(_)
            | Value::Integer(_)
            | Value::BigInteger(_)
            | Value::Float(_)
            | Value::Bool(_)
            | Value::Null
    )
}

/// `_format_inline` for a collection `_is_small_collection` accepts: under
/// five items, each value simple.
fn inline(value: &Value) -> Option<String> {
    match value {
        Value::List(items) if items.len() < MAX_INLINE_ITEMS && items.iter().all(is_simple) => {
            let items: Vec<String> = items.iter().map(model::pyrepr::py_str).collect();
            Some(format!("[{}]", items.join(", ")))
        }
        Value::Map(map)
            if map.len() < MAX_INLINE_ITEMS && map.iter().all(|(_, item)| is_simple(item)) =>
        {
            let items: Vec<String> = map
                .iter()
                .map(|(key, item)| format!("{key}: {}", model::pyrepr::py_str(item)))
                .collect();
            Some(format!("{{{}}}", items.join(", ")))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
