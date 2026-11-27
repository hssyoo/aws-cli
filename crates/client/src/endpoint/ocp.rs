//! `operationContextParams`: a rule parameter bound to what a JMESPath
//! path finds in the call's input
//! (`EndpointRulesetResolver._resolve_param_as_operation_context_param`).
//!
//! Admission still refuses every operation that declares one
//! ([`crate::admission::Unsupported::OperationContextParams`]), so only
//! callers that resolve without admission (the test façade) bind one yet.
//! A path outside Smithy's subset ([`jmespath::OPERATION_CONTEXT_PARAMS`]),
//! or one whose evaluation raises, leaves the parameter unset, where
//! botocore would raise.
use super::ParamValue;
use jmespath::{Expression, OPERATION_CONTEXT_PARAMS};
use model::schema::OperationSchema;
use model::value::Value;

pub use model::pyjmespath::Python;

/// Whether `operation` declares `name` in its `operationContextParams`.
pub(super) fn declares(operation: OperationSchema<'_>, name: &str) -> bool {
    path(operation, name).is_some()
}

fn path<'s>(operation: OperationSchema<'s>, name: &str) -> Option<&'s str> {
    let mut params = operation.operation_context_params();
    params
        .find(|(param, _)| *param == name)
        .map(|(_, path)| path)
}

/// `jmespath.search(path, input)` for the parameter `name`, coerced as
/// every context parameter is ([`super::param_value`]).
pub(super) fn param(
    operation: OperationSchema<'_>,
    input: &Value,
    name: &str,
) -> Option<ParamValue> {
    let expression = Expression::parse(&Python, path(operation, name)?).ok()?;
    expression.check(&OPERATION_CONTEXT_PARAMS).ok()?;
    super::param_value(&*expression.search(&Python, input).ok()?)
}

#[cfg(test)]
mod tests;
