//! Work in progress on this lane: the interface of `service_errors`
//! (T10e), which renders a model operation's failures as botocore's
//! handlers do. Each function keeps that crate's signature, so adopting it
//! changes only this module's callers' imports. Until then a failure keeps
//! `CallError`'s botocore text under the handler its kind belongs to.
use cli_core::cli_error::{Category, CommandFailure};
use client::error::CallError;
use model::shapes::Model;

/// The failure botocore's handler chain reports for `error`.
pub fn call_failure(error: &CallError, _model: &Model) -> CommandFailure {
    let category = match error {
        CallError::Validation(_) => Category::ParamValidation,
        CallError::NoRegion => Category::Configuration,
        CallError::NoCredentials => Category::NoCredentials,
        CallError::Service(_) => Category::ClientError,
        CallError::Interrupted => Category::Interrupt,
        _ => Category::General,
    };
    CommandFailure {
        message: error.to_string().into_bytes(),
        ..CommandFailure::new(category)
    }
}

/// The failure the pager's start reports: `PagerErrorHandler`'s for a
/// missing binary, the general handler's otherwise.
pub fn pager_failure(error: &format::SpawnError) -> CommandFailure {
    let category = if error.not_found {
        Category::Configuration
    } else {
        Category::General
    };
    CommandFailure {
        message: error.text.clone().into_bytes(),
        ..CommandFailure::new(category)
    }
}

/// A Ctrl-C that ended the command.
pub fn interrupt() -> CommandFailure {
    CommandFailure::new(Category::Interrupt)
}
