//! The call orchestrator: the phases of one operation call, in the order
//! they run. [`crate::call::Client`] owns the client and its public
//! surface (`new`, `prepare`, `send`, `call`); this module sequences the
//! phases those run. Positions that reach the wire, the error output or a
//! verdict are botocore's.
//!
//! [`configure`], once per client in `Client::new`: the customizations in
//! scope for the service ([`crate::runtime_plugin::ServicePlugins`]), the
//! one retry strategy (and so the one quota) all of the client's calls and
//! pages share, the region (`NoRegionError`) and the timeouts, held as the
//! client's [`crate::runtime_components`]. Operation scope is a filter
//! each phase applies, not a configuration of its own (decision D3).
//!
//! [`prepare`] (no identity, no transport; it may leave the call to the
//! Python route):
//! 1. `admit`: the static refusals and the endpoint configuration check;
//! 2. `modify_before_serialization` hooks;
//! 3. the idempotency tokens, after every input hook, then the checksum
//!    validation mode's default;
//! 4. `resolve_endpoint`: the built-ins after the
//!    `modify_endpoint_builtins` hooks, then the rule set, with the
//!    account id deferred;
//! 5. `serialize`: validation, then the protocol's serializer, then the
//!    call's checksums and their feature ids (`resolve_checksum_context`);
//! 6. `apply_endpoint`: the join with the endpoint and the `User-Agent`;
//! 7. `modify_before_retry_loop` hooks, then the request checksum
//!    (`apply_request_checksum`);
//! 8. `resolve_auth_scheme`: auth selection, a component.
//!
//! The checksum steps ([`crate::checksum`]) run only for a client whose
//! `http_checksum` switch is on, which no production client's is yet.
//!
//! [`send`]:
//! 1. `refresh_for_identity`: the deferred account id, which reruns
//!    phases 4, 6, 7 (with the request checksum) and 8 when it changes the
//!    endpoint;
//! 2. the invocation id; then per attempt:
//!    `modify_before_signing` hooks, identity and signature,
//!    `modify_before_transmit` (the core's unsigned headers, then the
//!    hooks), `transmit` (a `short_circuit_transmit` hook's answer, else
//!    the transport), `deserialize` (the response checksum, the
//!    `modify_before_deserialization` hooks, then the parsers), `classify`
//!    (the customizations' retry classifiers, else standard mode), the
//!    retry decision (with the long-poll wait), and on success
//!    `modify_before_completion` hooks.
//!
//! Not yet run, each until its first user: the
//! [`crate::interceptors::Intercept::observe`] points (the first is CLI
//! history, which today goes to Python) and
//! `modify_before_attempt_completion`, a core position with no core
//! finalizer yet.
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
