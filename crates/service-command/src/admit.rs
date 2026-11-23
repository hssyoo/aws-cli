//! Whether the native route runs a model operation, decided from the model,
//! the customization patterns and the completed parse, before any request.
//! The first refusal names its reason. What the model and the patterns
//! decide alone comes first (the protocol, the traits, then the
//! customizations), so an operation's verdict on those is the same whatever
//! its arguments; the arguments' binding and validation come last.
use crate::coverage;
use awscli_argparse::ParseResult;
use cli_core::cli_error::PythonReason;
use client::admission::Unsupported;
use client::endpoint::EndpointData;
use client::hooks::Customization;
use command::models::ModelSource;
use model::shapes::{Model, ShapeId, ShapeKind};
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
    let endpoints = operation(source, customizations, command, cli_operation)?;
    let lookup = || {
        let service = source.service(command)?;
        let table = source.table(command, cli_operation)?;
        Some((service, service.operation(cli_operation)?, table))
    };
    let Some((service, operation, table)) = lookup() else {
        return Err(PythonReason::UnsupportedSubcommand);
    };
    let input = crate::bind::bind(table, &parsed.values)?;
    let validation = validate_input(&service.model, operation, &input);
    if matches!(validation, Validation::Undecided(_)) {
        return Err(PythonReason::UnsupportedValue);
    }
    Ok(Admitted {
        command: command.to_owned(),
        service: service.name.clone(),
        operation: operation.name.to_string(),
        input,
        validation,
        endpoints,
    })
}

/// The stages decided by the model, its endpoint data and the patterns
/// alone, whatever the arguments, the configuration and the credentials:
/// the client's static refusals (`client::admission::static_refusal`: the
/// protocol, the operation's traits and keys the loader does not interpret,
/// context parameters, auth, required built-ins), a paginator (the CLI
/// pages, `CLIOperationCaller._make_client_call`), then the
/// customizations. Nothing here runs a credential provider. On success,
/// the endpoint data botocore's client creation loads.
pub fn operation(
    source: &ModelSource<'_>,
    customizations: &[&'static Customization],
    command: &str,
    cli_operation: &str,
) -> Result<EndpointData, PythonReason> {
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
    let refused = client::admission::static_refusal(
        model,
        operation,
        &endpoints,
        &service.name,
        customizations,
    );
    if let Some(unsupported) = refused {
        return Err(reason(&unsupported));
    }
    if service.paginated(&operation.name) {
        return Err(PythonReason::UnsupportedTrait);
    }
    if coverage::uncovered(&matches, customizations, &service.name, &operation.name).is_some() {
        return Err(PythonReason::UnportedCustomization);
    }
    Ok(endpoints)
}

/// The reason the Python route runs a call the client leaves to it, one
/// for each refusal.
pub fn reason(unsupported: &Unsupported) -> PythonReason {
    match unsupported {
        Unsupported::Protocol(_) => PythonReason::UnsupportedProtocol,
        Unsupported::Trait(_) => PythonReason::UnsupportedTrait,
        Unsupported::OperationContextParams => PythonReason::OperationContextParams,
        Unsupported::Builtin(_) => PythonReason::EndpointBuiltin,
        Unsupported::Auth(_) => PythonReason::UnsupportedAuth,
        Unsupported::UnsignedPayload => PythonReason::UnsignedPayload,
        Unsupported::EndpointHeaders => PythonReason::EndpointHeaders,
        Unsupported::RegionPattern => PythonReason::RegionPattern,
        Unsupported::EndpointData => PythonReason::EndpointData,
        Unsupported::Validation(_) => PythonReason::UnsupportedValue,
        Unsupported::Hook(_) => PythonReason::CustomizationHook,
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
