//! The services' own behaviour in the model client: what botocore keys by
//! service name, each a [`Customization`] scoped to its service by model
//! directory name, and `core`, which names the handlers every service runs
//! that the client's pipeline itself implements.
//!
//! [`TABLE`] is the table a client chooses from (`Hooks::for_service`).
#![forbid(unsafe_code)]
use client::hooks::{CallContext, Customization, Interceptor, Scope, SettingDefault};
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
///
/// and the handlers botocore registers on every client as it creates it
/// (`BaseClient._register_handlers`, `ClientCreator._register_retries`):
///
/// - `RequestSigner.handler`: the call's SigV4 signer, which also leaves
///   an `UNSIGNED` request (`--no-sign-request`, `noAuth`) unsigned;
/// - `UserAgentString.rebuild_and_replace_user_agent_handler`: the
///   User-Agent stage, with the feature ids the call registers;
/// - `MaxAttemptsSeeder.seed_max_attempts`: `amz-sdk-request`'s `max`,
///   from the resolved max attempts;
/// - `RetryHandler.needs_retry`: standard mode's retry conditions, backoff
///   and attempt limit;
/// - `RetryQuotaChecker.release_retry_quota`: the retry quota's refund on
///   success.
pub static CORE: Customization = Customization {
    id: "core",
    scope: Scope::ALL,
    setting_defaults: &[],
    builtins: &[],
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
        "botocore.signers.RequestSigner.handler",
        "botocore.useragent.UserAgentString.rebuild_and_replace_user_agent_handler",
        "botocore.retries.standard.MaxAttemptsSeeder.seed_max_attempts",
        "botocore.retries.standard.RetryHandler.needs_retry",
        "botocore.retries.standard.RetryQuotaChecker.release_retry_quota",
    ],
};

/// dynamodb's defaults: `retries/standard.py`'s `_SERVICE_MAX_ATTEMPTS`
/// (4) and `ExponentialBackoff._SERVICE_BACKOFF_CONFIG` (a 25 ms base for
/// errors that are not throttling), and `special.RetryDDBChecksumError`.
///
/// It replaces `block_endpoint_discovery_required_operations`, which a
/// dynamodb client registers (`_register_endpoint_discovery`: the service
/// has `DescribeEndpoints`, and discovery is `auto` and no operation
/// requires it): the handler raises `EndpointDiscoveryRequired` only for an
/// operation whose discovery is required (discovery.py:41-44), and none of
/// dynamodb's is; admission refuses any that were, and the router sends
/// any other `endpoint_discovery_enabled` to Python.
pub static DYNAMODB: Customization = Customization {
    id: "dynamodb",
    scope: Scope::service("dynamodb"),
    setting_defaults: &DYNAMODB_RETRIES,
    builtins: &[],
    interceptors: &[&DynamodbChecksum],
    replaces: &["botocore.discovery.block_endpoint_discovery_required_operations"],
};

/// dynamodb-streams, which the same two tables name; the checksum check is
/// dynamodb's only.
pub static DYNAMODB_STREAMS: Customization = Customization {
    id: "dynamodbstreams",
    scope: Scope::service("dynamodbstreams"),
    setting_defaults: &DYNAMODB_RETRIES,
    builtins: &[],
    interceptors: &[],
    replaces: &[],
};

const DYNAMODB_RETRIES: [SettingDefault; 2] = [
    SettingDefault::MaxAttempts(4),
    SettingDefault::RetryBaseDelay(Duration::from_millis(25)),
];

/// sts: `special.RetryIDPCommunicationError`. (Its rule set's
/// `AWS::STS::UseGlobalEndpoint` built-in is core's: botocore gives every
/// client `False`, `compute_endpoint_resolver_builtin_defaults`.)
/// AssumeRoleWithSAML and
/// AssumeRoleWithWebIdentity go unsigned by their models' `noAuth`, which
/// is what `disable_signing` makes them.
pub static STS: Customization = Customization {
    id: "sts",
    scope: Scope::service("sts"),
    setting_defaults: &[],
    builtins: &[],
    interceptors: &[&StsIdpRetry],
    replaces: &["botocore.handlers.disable_signing"],
};

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
        (error.code() == Some("IDPCommunicationError")).then_some(RetryDecision::Retry)
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
