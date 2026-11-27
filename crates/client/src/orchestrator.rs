//! The call orchestrator: the phases of one operation call, in the order
//! they run. [`crate::call::Client`] owns the client and its public
//! surface (`new`, `prepare`, `send`, `call`); this module sequences the
//! phases those run. Positions that reach the wire, the error output or a
//! verdict are botocore's.
//!
//! [`configure`], once per client in `Client::new`: the customizations in
//! scope for the service, the one retry strategy (and so the one quota)
//! all of the client's calls and pages share, the region (`NoRegionError`)
//! and the timeouts, held as the client's [`crate::runtime_components`].
//! Operation scope is a filter each phase applies, not a configuration
//! of its own (decision D3).
//!
//! [`prepare`] (no identity, no transport; it may leave the call to the
//! Python route):
//! 1. `admit`: the static refusals and the endpoint configuration check;
//! 2. `modify_before_serialization`: the `modify_input` stage;
//! 3. the idempotency tokens, after every input hook;
//! 4. `resolve_endpoint`: the built-ins after the `endpoint_builtins`
//!    stage, then the rule set, with the account id deferred;
//! 5. `serialize`: validation, then the protocol's serializer;
//! 6. `apply_endpoint`: the join with the endpoint and the `User-Agent`;
//! 7. `modify_before_retry_loop`: the `after_serialize` stage;
//! 8. `resolve_auth_scheme`: auth selection and the `select_auth` stage.
//!
//! [`send`]:
//! 1. `refresh_for_identity`: the deferred account id, which reruns
//!    phases 4, 6, 7 and 8 when it changes the endpoint;
//! 2. the invocation id; then per attempt:
//!    `modify_before_signing` (the `before_sign` stage), identity and
//!    signature, `modify_before_transmit` (the core's unsigned headers),
//!    `transmit` (the `before_transmit` stage's answer, else the
//!    transport), `deserialize` (the `after_receive` stage, then the
//!    parsers), `classify` (the `classify_retry` stage, else standard
//!    mode), the retry decision (with the long-poll wait), and on success
//!    `modify_before_completion` (the `modify_output` stage).
use crate::admission::Unsupported;
use crate::error::CallError;
use crate::hooks::HookError;
use model::schema::OperationSchema;
use model::shapes::Model;

mod configure;
mod prepare;
mod send;

pub(crate) use configure::configure;
pub(crate) use prepare::prepare;
pub(crate) use send::send;

/// The operation of `model` named `name`.
fn find<'m>(model: &'m Model, name: &str) -> Result<OperationSchema<'m>, CallError> {
    (model.schema().service())
        .operation(name)
        .ok_or_else(|| CallError::UnknownOperation(name.to_owned()))
}

/// A `prepare` phase's hook error: a refusal leaves the call to Python,
/// which is legal there because nothing has been sent and no credential
/// provider has run.
fn prepare_hook_error(error: HookError) -> CallError {
    match error {
        HookError::Refuse(reason) => CallError::Unsupported(Unsupported::Hook(reason)),
        other => CallError::Hook(other),
    }
}

/// A `prepare` phase's error when [`send`] runs it again, after a
/// credential provider has run, too late to leave the call to Python: a
/// customization's refusal fails the call ([`HookError::after_prepare`]),
/// and any other refusal is a defect of the client, named as one.
fn after_prepare(error: CallError) -> CallError {
    match error {
        CallError::Unsupported(Unsupported::Hook(reason)) => {
            CallError::Hook(HookError::Refuse(reason).after_prepare())
        }
        CallError::Unsupported(unsupported) => CallError::Internal(format!(
            "the call was refused after credentials had loaded, which prepare should have \
             done: {unsupported}"
        )),
        other => other,
    }
}
