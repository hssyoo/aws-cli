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
use model::shapes::{Model, ShapeId, ShapeKind};
use model::validate::{Validation, validate_input};
use model::value::{Map, Value};

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
    /// What an `operation-args-parsed` handler raised before the request
    /// was built, which the invocation reports in place of any call.
    pub raised: Option<CommandFailure>,
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
) -> Result<Admitted, PythonReason> {
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
    let mut admitted = Admitted {
        command: command.to_owned(),
        service: service.name.clone(),
        operation: operation.name.to_string(),
        input: Value::Map(Map::new()),
        validation: Validation::Valid,
        endpoints: decided.endpoints,
        paginated: None,
        warned: false,
        raised: None,
    };
    let call = match crate::bind::bind(table, parsed)? {
        Binding::Call(call) => call,
        Binding::Raised(failure) => {
            admitted.raised = Some(failure);
            return Ok(admitted);
        }
    };
    let validation = validate_input(&service.model, operation, &call.input);
    if matches!(validation, Validation::Undecided(_)) {
        return Err(PythonReason::UnsupportedValue);
    }
    admitted.paginated = decided
        .paginator
        .zip(call.pagination)
        .map(|(config, pagination)| Paginated { config, pagination });
    admitted.input = call.input;
    admitted.validation = validation;
    admitted.warned = call.warned;
    Ok(admitted)
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
    let endpoints = EndpointData::load(source.search(), &service.name, model.metadata())
        .map_err(|_| PythonReason::EndpointData)?;
    let entry = service.paginator(&operation.name);
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
    if coverage::uncovered(&matches, customizations, &service.name, &operation.name).is_some() {
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
pub fn reaches_epoch_timestamp(model: &Model, shape: ShapeId) -> bool {
    let json = client::protocol::epoch_timestamps(model).unwrap_or(false);
    reaches(model, shape, &mut Vec::new(), &|found| {
        matches!(found.kind, ShapeKind::Timestamp)
            && match found.traits.timestamp_format.as_deref() {
                Some(format) => format == "unixTimestamp",
                None => json,
            }
    })
}

fn reaches(
    model: &Model,
    shape: ShapeId,
    seen: &mut Vec<ShapeId>,
    wanted: &dyn Fn(&model::shapes::Shape) -> bool,
) -> bool {
    if seen.contains(&shape) {
        return false;
    }
    seen.push(shape);
    let found = model.shape(shape);
    if wanted(found) {
        return true;
    }
    match found.kind {
        ShapeKind::Structure { .. } => model
            .members(shape)
            .iter()
            .any(|member| reaches(model, member.shape, seen, wanted)),
        ShapeKind::List { member } => reaches(model, member, seen, wanted),
        ShapeKind::Map { key, value } => {
            reaches(model, key, seen, wanted) || reaches(model, value, seen, wanted)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
