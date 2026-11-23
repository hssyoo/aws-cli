//! Whether the native route runs a model operation, decided from the model,
//! the customization patterns and the completed parse, before any request.
//! The first refusal names its reason; the order is the plan's (PLAN §2.2,
//! native eligibility).
use crate::coverage;
use awscli_argparse::ParseResult;
use cli_core::cli_error::PythonReason;
use client::hooks::Customization;
use command::models::{ModelSource, Service};
use model::shapes::{Flag, Model, Operation, ShapeId, ShapeKind};
use model::validate::{Validation, validate_input};
use model::value::Value;

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
    let lookup = || {
        let service = source.service(command)?;
        let table = source.table(command, cli_operation)?;
        let matches = source.matches(command, cli_operation)?;
        Some((service, service.operation(cli_operation)?, table, matches))
    };
    let Some((service, operation, table, matches)) = lookup() else {
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
    })
}

/// A trait of the operation the native route does not implement: a
/// paginator (`CLIOperationCaller._make_client_call` paginates), an event
/// stream or a streaming member at either end (`has_event_stream_input`,
/// `has_streaming_output` and their kin read the top-level members), or a
/// document anywhere in its input or output, which neither the binding nor
/// the formatter carries.
pub fn unimplemented_trait(service: &Service, operation: &Operation) -> bool {
    let model = &service.model;
    service.paginated(&operation.name)
        || [operation.input, operation.output]
            .into_iter()
            .flatten()
            .any(|shape| {
                streams(model, shape)
                    || reaches(model, shape, &mut Vec::new(), &|found| {
                        found.traits.is(Flag::Document)
                    })
            })
}

fn streams(model: &Model, shape: ShapeId) -> bool {
    model.members(shape).iter().any(|member| {
        let traits = &model.shape(member.shape).traits;
        traits.is(Flag::Streaming) || traits.is(Flag::EventStream)
    })
}

/// Whether `shape` reaches a timestamp member, whose wire value the
/// session's timestamp parser reads.
pub fn reaches_timestamp(model: &Model, shape: ShapeId) -> bool {
    reaches(model, shape, &mut Vec::new(), &|found| {
        matches!(found.kind, ShapeKind::Timestamp)
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
