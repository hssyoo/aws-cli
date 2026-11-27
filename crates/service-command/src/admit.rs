//! Whether the native route runs a model operation, decided from the model,
//! the customization patterns and the completed parse, before any request.
//! The first refusal names its reason. What the model and the patterns
//! decide alone comes first (the protocol, the traits, then the
//! customizations), so an operation's verdict on those is the same whatever
//! its arguments; the arguments' binding and validation come last.
use crate::bind::Binding;
use crate::coverage;
use awscli_argparse::ParseResult;
use cli_core::cli_error::{CommandFailure, PythonReason};
use client::admission::Unsupported;
use client::endpoint::EndpointData;
use client::hooks::Customization;
use client::paginate::PaginationConfig;
use command::models::ModelSource;
use model::paginators::Config;
use model::schema::{Schema, ShapeType};
use model::shapes::Model;
use model::validate::{Validation, validate_input};
use model::value::Value;

/// An admitted invocation: what it calls, with what.
#[derive(Debug, Clone)]
pub struct Admitted {
    /// The root command, as the CLI names it.
    pub command: String,
    /// The model's directory name, botocore's `service_name`.
    pub service: String,
    /// The operation's model name.
    pub operation: String,
    pub input: Value,
    /// The client's validation of `input`: `Valid`, or the report botocore
    /// would raise, which the invocation renders. Never `Undecided`.
    pub validation: Validation,
    /// The endpoint data the client is built with.
    pub endpoints: EndpointData,
    /// The paginator, when `_make_client_call` pages: the operation has one
    /// (`client.can_paginate`) and paging stayed on
    /// (`parsed_globals.paginate`).
    pub paginated: Option<Paginated>,
    /// `PageArgument.add_to_params` warns that `--max-items` is not
    /// positive, before the call.
    pub warned: bool,
}

/// What admission comes to for a parse it does not leave to Python.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// The call to make.
    Call(Box<Admitted>),
    /// What an `operation-args-parsed` handler of `command` raised
    /// (`ensure_paging_params_not_set`), before botocore builds the
    /// request or creates a client: the invocation reports it and makes
    /// neither.
    Raised {
        command: String,
        failure: CommandFailure,
    },
}

/// `Paginator.paginate(**input, PaginationConfig=pagination)`'s paginator
/// configuration and options.
#[derive(Debug, Clone, PartialEq)]
pub struct Paginated {
    pub config: Config,
    pub pagination: PaginationConfig,
}

/// Admit the completed parse of `[command, operation]`, or name why Python
/// runs it. `customizations` are the client's, which cover the botocore
/// rows.
pub fn admit(
    source: &ModelSource<'_>,
    customizations: &[&'static Customization],
    parsed: &ParseResult<'_>,
) -> Result<Outcome, PythonReason> {
    let [command, cli_operation] = parsed.lineage.as_slice() else {
        return Err(PythonReason::UnsupportedSubcommand);
    };
    let (command, cli_operation) = (&*command.name, &*cli_operation.name);
    let decided = operation(source, customizations, command, cli_operation)?;
    let lookup = || {
        let service = source.service(command)?;
        let table = source.table(command, cli_operation)?;
        Some((service, service.operation(cli_operation)?, table))
    };
    let Some((service, operation, table)) = lookup() else {
        return Err(PythonReason::UnsupportedSubcommand);
    };
    let call = match crate::bind::bind(table, parsed)? {
        Binding::Call(call) => call,
        Binding::Raised(failure) => {
            return Ok(Outcome::Raised {
                command: command.to_owned(),
                failure,
            });
        }
    };
    let validation = validate_input(operation, &call.input);
    if matches!(validation, Validation::Undecided(_)) {
        return Err(PythonReason::UnsupportedValue);
    }
    let paginated = decided
        .paginator
        .zip(call.pagination)
        .map(|(config, pagination)| Paginated { config, pagination });
    Ok(Outcome::Call(Box::new(Admitted {
        command: command.to_owned(),
        service: service.name.clone(),
        operation: operation.name().to_owned(),
        input: call.input,
        validation,
        endpoints: decided.endpoints,
        paginated,
        warned: call.warned,
    })))
}

/// The stages decided by the model, its endpoint data and the patterns
/// alone, whatever the arguments, the configuration and the credentials:
/// the client's static refusals (`client::admission::static_refusal`: the
/// protocol, the operation's traits and keys the loader does not interpret,
/// context parameters, auth, required built-ins), a paginator the native
/// one does not run (the CLI pages, `CLIOperationCaller._make_client_call`),
/// then the customizations. Nothing here runs a credential provider.
pub fn operation(
    source: &ModelSource<'_>,
    customizations: &[&'static Customization],
    command: &str,
    cli_operation: &str,
) -> Result<Decided, PythonReason> {
    let lookup = || {
        let service = source.service(command)?;
        let matches = source.matches(command, cli_operation)?;
        Some((service, service.operation(cli_operation)?, matches))
    };
    let Some((service, operation, matches)) = lookup() else {
        return Err(PythonReason::UnsupportedSubcommand);
    };
    let model = &service.model;
    let endpoints = EndpointData::load(source.search(), &service.name, model.schema().service())
        .map_err(|_| PythonReason::EndpointData)?;
    let entry = service.paginator(operation.name());
    let refused = client::admission::static_refusal(
        model,
        operation,
        &endpoints,
        &service.name,
        customizations,
        entry.map(|entry| &entry.config),
    );
    if let Some(unsupported) = refused {
        return Err(reason(&unsupported));
    }
    let paginator = match entry.map(|entry| &entry.config) {
        None => None,
        Some(Ok(config)) => Some(config.clone()),
        // What `static_refusal` refuses.
        Some(Err(_)) => return Err(PythonReason::Paginator),
    };
    // unify_paging_params raises as it builds a table without the paging
    // options.
    let paged = || {
        source
            .table(command, cli_operation)
            .is_some_and(|table| table.paging.is_some())
    };
    if entry.is_some() && !paged() {
        return Err(PythonReason::Paginator);
    }
    if coverage::uncovered(&matches, customizations, &service.name, operation.name()).is_some() {
        return Err(PythonReason::UnportedCustomization);
    }
    Ok(Decided {
        endpoints,
        paginator,
    })
}

/// What the stages [`operation`] runs leave for the call: the endpoint
/// data botocore's client creation loads, and the operation's paginator,
/// parsed once.
#[derive(Debug, Clone)]
pub struct Decided {
    pub endpoints: EndpointData,
    pub paginator: Option<Config>,
}

/// The reason the Python route runs a call the client leaves to it, one
/// for each refusal.
pub fn reason(unsupported: &Unsupported) -> PythonReason {
    match unsupported {
        Unsupported::Protocol(_) => PythonReason::UnsupportedProtocol,
        Unsupported::Trait(_) => PythonReason::UnsupportedTrait,
        Unsupported::OperationContextParams => PythonReason::OperationContextParams,
        Unsupported::Builtin(_) => PythonReason::EndpointBuiltin,
        Unsupported::ClientContextParam(_) => PythonReason::ClientContextParam,
        Unsupported::Auth(_) => PythonReason::UnsupportedAuth,
        Unsupported::UnsignedPayload => PythonReason::UnsignedPayload,
        Unsupported::EndpointHeaders => PythonReason::EndpointHeaders,
        Unsupported::RegionPattern => PythonReason::RegionPattern,
        Unsupported::EndpointData => PythonReason::EndpointData,
        Unsupported::Validation(_) => PythonReason::UnsupportedValue,
        Unsupported::Hook(_) => PythonReason::CustomizationHook,
        Unsupported::StartingToken(_) => PythonReason::StartingToken,
        Unsupported::Paginator(_) => PythonReason::Paginator,
    }
}

/// Whether `shape` reaches a timestamp the wire sends as an epoch, which
/// `iso8601` output reads in the local zone (`parse_timestamp` of a
/// number): a member or shape whose `timestampFormat` is `unixTimestamp`,
/// or one with none under a protocol whose default it is
/// (`client::protocol::epoch_timestamps`). ISO 8601 and RFC 822 text
/// carries its own offset.
pub fn reaches_epoch_timestamp(model: &Model, shape: Schema<'_>) -> bool {
    let json = client::protocol::epoch_timestamps(model).unwrap_or(false);
    reaches(shape, &mut Vec::new(), &|found| {
        found.shape_type() == ShapeType::Timestamp
            && match found.timestamp_format_text() {
                Some(format) => format == "unixTimestamp",
                None => json,
            }
    })
}

/// Whether `wanted` holds for `shape` or a schema it reaches through
/// members, list members, map keys and values, whatever its type says.
fn reaches<'s>(
    shape: Schema<'s>,
    seen: &mut Vec<Schema<'s>>,
    wanted: &dyn Fn(Schema<'s>) -> bool,
) -> bool {
    if seen.contains(&shape) {
        return false;
    }
    seen.push(shape);
    if wanted(shape) {
        return true;
    }
    let children = shape
        .list_member()
        .into_iter()
        .chain(shape.map_key())
        .chain(shape.map_value());
    shape
        .members()
        .chain(children)
        .any(|child| reaches(child, seen, wanted))
}

#[cfg(test)]
mod tests;
