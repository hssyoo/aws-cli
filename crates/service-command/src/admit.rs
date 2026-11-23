//! Whether the native route runs a model operation, decided from the model,
//! the customization patterns and the completed parse, before any request.
//! The first refusal names its reason. What the model and the patterns
//! decide alone comes first (the protocol, the traits, then the
//! customizations), so an operation's verdict on those is the same whatever
//! its arguments; the arguments' binding and validation come last.
use crate::bind::Bound;
use crate::coverage;
use awscli_argparse::ParseResult;
use cli_core::cli_error::{CommandFailure, PythonReason};
use client::hooks::Customization;
use client::paginate::{Config, PaginationConfig, Refused};
use command::models::{ModelSource, Service};
use model::shapes::{Flag, Model, Operation, ShapeId, ShapeKind};
use model::validate::{Validation, validate_input};
use model::value::{Map, Value};

/// An admitted invocation: what it calls, with what.
#[derive(Debug, Clone, PartialEq)]
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
    customizations: &[&Customization],
    parsed: &ParseResult<'_>,
) -> Result<Admitted, PythonReason> {
    let [command, cli_operation] = parsed.lineage.as_slice() else {
        return Err(PythonReason::UnsupportedSubcommand);
    };
    let (command, cli_operation) = (&*command.name, &*cli_operation.name);
    operation(source, customizations, command, cli_operation)?;
    let lookup = || {
        let service = source.service(command)?;
        let table = source.table(command, cli_operation)?;
        Some((service, service.operation(cli_operation)?, table))
    };
    let Some((service, operation, table)) = lookup() else {
        return Err(PythonReason::UnsupportedSubcommand);
    };
    let config = paginator(service, operation).transpose();
    let Ok(config) = config else {
        return Err(PythonReason::UnsupportedTrait);
    };
    if config.is_some() && table.paging.is_none() {
        // unify_paging_params raises as it builds the table.
        return Err(PythonReason::UnsupportedTrait);
    }
    let mut admitted = Admitted {
        command: command.to_owned(),
        service: service.name.clone(),
        operation: operation.name.to_string(),
        input: Value::Map(Map::new()),
        validation: Validation::Valid,
        paginated: None,
        warned: false,
        raised: None,
    };
    let call = match crate::bind::bind(table, parsed)? {
        Bound::Call(call) => call,
        Bound::Raised(failure) => {
            admitted.raised = Some(failure);
            return Ok(admitted);
        }
    };
    let validation = validate_input(&service.model, operation, &call.input);
    if matches!(validation, Validation::Undecided(_)) {
        return Err(PythonReason::UnsupportedValue);
    }
    admitted.paginated = config
        .zip(call.pagination)
        .map(|(config, pagination)| Paginated { config, pagination });
    admitted.input = call.input;
    admitted.validation = validation;
    admitted.warned = call.warned;
    Ok(admitted)
}

/// The stages decided by the model and the patterns alone, whatever the
/// arguments: the protocol, the traits, then the customizations.
pub fn operation(
    source: &ModelSource<'_>,
    customizations: &[&Customization],
    command: &str,
    cli_operation: &str,
) -> Result<(), PythonReason> {
    let lookup = || {
        let service = source.service(command)?;
        let matches = source.matches(command, cli_operation)?;
        Some((service, service.operation(cli_operation)?, matches))
    };
    let Some((service, operation, matches)) = lookup() else {
        return Err(PythonReason::UnsupportedSubcommand);
    };
    if client::protocol::for_model(&service.model).is_none() {
        return Err(PythonReason::UnsupportedProtocol);
    }
    if unimplemented_trait(service, operation) {
        return Err(PythonReason::UnsupportedTrait);
    }
    if coverage::uncovered(&matches, customizations, &service.name, &operation.name).is_some() {
        return Err(PythonReason::UnportedCustomization);
    }
    Ok(())
}

/// The operation's paginator, as `client.get_paginator` reads its entry in
/// the paginators file; `None` when it has none (`client.can_paginate`).
///
/// # Errors
/// [`Refused`]: an entry the native paginator does not run.
pub fn paginator(service: &Service, operation: &Operation) -> Option<Result<Config, Refused>> {
    let entry = service.paginator(&operation.name)?;
    Some(Config::from_value(&Value::from_serde(entry)))
}

/// A trait of the operation the native route does not implement: a
/// paginator whose entry the native paginator refuses (an expression
/// beyond member names and dotted paths), an event stream, a streaming
/// member or a blob payload at either end (`has_event_stream_input`,
/// `has_streaming_output` and their kin read the top-level members and the
/// payload), or one the client refuses before any request
/// (`client::admission::unimplemented_trait`: request checksums, required
/// endpoint discovery, the endpoint operation, request compression). A
/// document member is no refusal: the json protocols carry it both ways,
/// the formatter prints it, and a supplied one is deferred at binding.
pub fn unimplemented_trait(service: &Service, operation: &Operation) -> bool {
    let model = &service.model;
    client::admission::unimplemented_trait(operation).is_some()
        || paginator(service, operation).is_some_and(|config| config.is_err())
        || [operation.input, operation.output]
            .into_iter()
            .flatten()
            .any(|shape| streams(model, shape))
}

fn streams(model: &Model, shape: ShapeId) -> bool {
    model.members(shape).iter().any(|member| {
        let traits = &model.shape(member.shape).traits;
        traits.is(Flag::Streaming) || traits.is(Flag::EventStream)
    }) || blob_payload(model, shape)
}

/// `OperationModel._get_streaming_body`: a `payload` member that is a
/// blob streams whether or not it is marked `streaming`, so botocore
/// returns it as a `StreamingBody` and the CLI writes it to `outfile`.
fn blob_payload(model: &Model, shape: ShapeId) -> bool {
    let Some(payload) = model.shape(shape).traits.payload.as_deref() else {
        return false;
    };
    model
        .members(shape)
        .iter()
        .find(|member| &*member.name == payload)
        .is_some_and(|member| matches!(model.shape(member.shape).kind, ShapeKind::Blob))
}

/// Whether `shape` reaches a timestamp the wire sends as an epoch, which
/// `iso8601` output reads in the local zone (`parse_timestamp` of a
/// number): a member or shape whose `timestampFormat` is `unixTimestamp`,
/// or one with none under a json protocol, whose default it is. ISO 8601
/// and RFC 822 text carries its own offset, and is the XML protocols'
/// default.
pub fn reaches_epoch_timestamp(model: &Model, shape: ShapeId) -> bool {
    let json = client::protocol::resolved_protocol(model.metadata())
        .is_ok_and(|protocol| matches!(protocol, "json" | "rest-json"));
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
