//! The services' own behaviour in the model client: what botocore keys by
//! service name, each a [`Customization`] scoped to its service by model
//! directory name, and `core`, which names the handlers every service runs
//! that the client's pipeline itself implements.
//!
//! [`TABLE`] is the table a client chooses from (`Hooks::for_service`).
#![forbid(unsafe_code)]
use client::endpoint::{EndpointParams, ParamValue};
use client::hooks::{CallContext, Customization, HookResult, Interceptor, Scope, SettingDefault};
use client::retry::{Attempt, RetryDecision};
use std::time::Duration;

/// Every customization, in the order a client applies them.
pub static TABLE: [&Customization; 4] = [&CORE, &DYNAMODB, &DYNAMODB_STREAMS, &STS];

/// The botocore handlers registered for every service
/// (`botocore.handlers.BUILTIN_HANDLERS`, the rows without a service in
/// their event) that the client's pipeline implements, or whose effect on
/// a query or json call it reproduces by refusing what they act on:
///
/// - `generate_idempotent_uuid`: the call loop fills idempotency tokens;
/// - `add_retry_headers`: it adds `amz-sdk-invocation-id` and
///   `amz-sdk-request`;
/// - `set_operation_specific_signer`: auth selection;
/// - `_set_auth_scheme_preference_signer`: does nothing without
///   `auth_scheme_preference`, which the router sends to Python;
/// - `_handle_request_validation_mode_member`: acts only on `httpChecksum`
///   operations, which the client refuses;
/// - `inject_api_version_header_if_needed`: acts only on endpoint
///   discovery operations, which the client refuses;
/// - `handle_service_name_alias`: the command layer names the service by
///   its model directory, so no alias reaches a client;
/// - `add_generate_presigned_url`: adds a method the CLI's service
///   commands never call.
pub static CORE: Customization = Customization {
    id: "core",
    scope: Scope::ALL,
    setting_defaults: &[],
    interceptors: &[],
    replaces: &[
        "botocore.handlers.generate_idempotent_uuid",
        "botocore.handlers.add_retry_headers",
        "botocore.handlers.set_operation_specific_signer",
        "botocore.handlers._set_auth_scheme_preference_signer",
        "botocore.handlers._handle_request_validation_mode_member",
        "botocore.handlers.inject_api_version_header_if_needed",
        "botocore.handlers.handle_service_name_alias",
        "botocore.signers.add_generate_presigned_url",
    ],
};

/// dynamodb's defaults: `retries/standard.py`'s `_SERVICE_MAX_ATTEMPTS`
/// (4) and `ExponentialBackoff._SERVICE_BACKOFF_CONFIG` (a 25 ms base for
/// errors that are not throttling), and `special.RetryDDBChecksumError`.
pub static DYNAMODB: Customization = Customization {
    id: "dynamodb",
    scope: Scope::service("dynamodb"),
    setting_defaults: &DYNAMODB_RETRIES,
    interceptors: &[&DynamodbChecksum],
    replaces: &[],
};

/// dynamodb-streams, which the same two tables name; the checksum check is
/// dynamodb's only.
pub static DYNAMODB_STREAMS: Customization = Customization {
    id: "dynamodbstreams",
    scope: Scope::service("dynamodbstreams"),
    setting_defaults: &DYNAMODB_RETRIES,
    interceptors: &[],
    replaces: &[],
};

const DYNAMODB_RETRIES: [SettingDefault; 2] = [
    SettingDefault::MaxAttempts(4),
    SettingDefault::RetryBaseDelay(Duration::from_millis(25)),
];

/// sts: the `AWS::STS::UseGlobalEndpoint` built-in, false
/// (`compute_endpoint_resolver_builtin_defaults`), and
/// `special.RetryIDPCommunicationError`. AssumeRoleWithSAML and
/// AssumeRoleWithWebIdentity go unsigned by their models' `noAuth`, which
/// is what `disable_signing` makes them.
pub static STS: Customization = Customization {
    id: "sts",
    scope: Scope::service("sts"),
    setting_defaults: &[],
    interceptors: &[&StsGlobalEndpoint, &StsIdpRetry],
    replaces: &["botocore.handlers.disable_signing"],
};

#[derive(Debug)]
struct StsGlobalEndpoint;

impl Interceptor for StsGlobalEndpoint {
    fn endpoint_params(&self, _: &mut CallContext<'_>, params: &mut EndpointParams) -> HookResult {
        params.set("UseGlobalEndpoint", ParamValue::Boolean(false));
        Ok(())
    }
}

#[derive(Debug)]
struct StsIdpRetry;

impl Interceptor for StsIdpRetry {
    fn classify_retry(
        &self,
        _: &mut CallContext<'_>,
        attempt: &Attempt<'_>,
    ) -> Option<RetryDecision> {
        let Attempt::Response {
            error: Some(error), ..
        } = attempt
        else {
            return None;
        };
        (error.code.as_deref() == Some("IDPCommunicationError")).then_some(RetryDecision::Retry)
    }
}

#[derive(Debug)]
struct DynamodbChecksum;

impl Interceptor for DynamodbChecksum {
    /// `RetryDDBChecksumError.is_retryable`: the body's CRC32 differs from
    /// `x-amz-crc32`. A header that is not a number botocore cannot read
    /// either; it is left to the standard policy.
    fn classify_retry(
        &self,
        _: &mut CallContext<'_>,
        attempt: &Attempt<'_>,
    ) -> Option<RetryDecision> {
        let Attempt::Response { response, .. } = attempt else {
            return None;
        };
        let expected: u64 = response.headers.get("x-amz-crc32")?.trim().parse().ok()?;
        (u64::from(crc32(&response.body)) != expected).then_some(RetryDecision::Retry)
    }
}

/// `binascii.crc32`: CRC-32 (IEEE 802.3), bit by bit.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests;
