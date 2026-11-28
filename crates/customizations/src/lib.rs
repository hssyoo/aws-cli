//! The services' own behaviour in the model client: what botocore keys by
//! service name, each a [`Customization`] scoped to its service by model
//! directory name, and `core`, which names the handlers every service runs
//! that the client's pipeline itself implements.
//!
//! [`TABLE`] is the table a client chooses from
//! (`ServicePlugins::for_service`).
#![forbid(unsafe_code)]
use cli_core::cli_error::PythonReason;
use client::hooks::{Basis, Customization, HookError, Refusal, Scope, SettingDefault};
use client::interceptors::InterceptorContext;
use client::retry::{Attempt, RetryDecision};
use client::runtime_components::ClassifyRetry;
use std::time::Duration;

/// A basis implemented at the Rust item `at`.
const fn implemented(at: &'static str) -> Basis {
    Basis::Implemented { at }
}

/// Every customization, in the order a client applies them.
pub static TABLE: [&Customization; 9] = [
    &CORE,
    &EC2,
    &DYNAMODB,
    &DYNAMODB_STREAMS,
    &STS,
    &SQS_RECEIVE_MESSAGE,
    &SFN_GET_ACTIVITY_TASK,
    &SWF_POLL_FOR_ACTIVITY_TASK,
    &SWF_POLL_FOR_DECISION_TASK,
];

/// The botocore handlers registered for every service
/// (`botocore.handlers.BUILTIN_HANDLERS`, the rows without a service in
/// their event) that the client's pipeline implements, or whose effect on
/// a query, json or rest-json call it reproduces by refusing what they act
/// on:
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
/// - `add_generate_presigned_url`, and rds's and dsql's
///   `add_generate_db_auth_token` and
///   `add_dsql_generate_db_auth_token_methods` (on their services'
///   `creating-client-class`): add methods the CLI's service commands
///   never call.
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
    retry_classifiers: &[],
    long_polling: false,
    replaces: &[
        (
            "botocore.handlers.generate_idempotent_uuid",
            implemented("client::orchestrator::prepare::fill_idempotency_tokens"),
        ),
        (
            "botocore.handlers.add_retry_headers",
            implemented("client::orchestrator::send::modify_before_transmit"),
        ),
        (
            "botocore.handlers.set_operation_specific_signer",
            implemented("client::auth::select"),
        ),
        (
            "botocore.handlers._set_auth_scheme_preference_signer",
            Basis::Inert {
                acts_when: "auth_scheme_preference is configured",
                refused_as: Refusal::Router(PythonReason::ClientSetting),
            },
        ),
        (
            "botocore.handlers._handle_request_validation_mode_member",
            Basis::Inert {
                acts_when: "the operation has httpChecksum",
                refused_as: Refusal::Trait("httpChecksum"),
            },
        ),
        (
            "botocore.handlers.inject_api_version_header_if_needed",
            Basis::Inert {
                acts_when: "the operation is the endpoint-discovery operation",
                refused_as: Refusal::Trait("endpointoperation"),
            },
        ),
        (
            "botocore.handlers.handle_service_name_alias",
            implemented("command::models::ModelSource::roots"),
        ),
        (
            "botocore.signers.add_generate_presigned_url",
            Basis::Inert {
                acts_when: "a hand-written command calls generate_presigned_url (s3 presign, \
                            eks get-token, ec2-instance-connect, cloudfront sign)",
                refused_as: Refusal::Router(PythonReason::UnsupportedCommand),
            },
        ),
        (
            "botocore.signers.add_generate_db_auth_token",
            Basis::Inert {
                acts_when: "rds generate-db-auth-token, a hand-written command, calls the method",
                refused_as: Refusal::Router(PythonReason::UnsupportedCommand),
            },
        ),
        (
            "botocore.signers.add_dsql_generate_db_auth_token_methods",
            Basis::Inert {
                acts_when: "dsql's generate-db-connect-auth-token or \
                            generate-db-connect-admin-auth-token, hand-written commands, \
                            call the methods",
                refused_as: Refusal::Router(PythonReason::UnsupportedCommand),
            },
        ),
        (
            "botocore.signers.RequestSigner.handler",
            implemented("client::auth::sign"),
        ),
        (
            "botocore.useragent.UserAgentString.rebuild_and_replace_user_agent_handler",
            implemented("client::useragent::header"),
        ),
        (
            "botocore.retries.standard.MaxAttemptsSeeder.seed_max_attempts",
            implemented("client::orchestrator::send::modify_before_transmit"),
        ),
        (
            "botocore.retries.standard.RetryHandler.needs_retry",
            implemented("client::retry::Standard::needs_retry"),
        ),
        (
            "botocore.retries.standard.RetryQuotaChecker.release_retry_quota",
            implemented("client::retry::Standard::succeeded"),
        ),
    ],
};

/// ec2: botocore's `ParameterAlias('Filter', 'Filters')` on
/// `before-parameter-build.ec2.*` (`_add_parameter_aliases`). It moves a
/// `Filters` key of the call's params to `Filter`, and only for an
/// operation whose input has a `Filter` member. A CLI parse keys the
/// params by input member, and no ec2 input has both members (a test
/// checks), so only `--cli-input-json` can give it a `Filters` to move;
/// the router sends that option to Python.
pub static EC2: Customization = Customization {
    id: "ec2",
    scope: Scope::service("ec2"),
    setting_defaults: &[],
    builtins: &[],
    interceptors: &[],
    retry_classifiers: &[],
    long_polling: false,
    replaces: &[(
        "botocore.handlers.ParameterAlias.alias_parameter_in_call",
        Basis::Inert {
            acts_when: "--cli-input-json gives Filters for an operation whose input has Filter",
            refused_as: Refusal::Router(PythonReason::UnsupportedOption),
        },
    )],
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
    interceptors: &[],
    retry_classifiers: &[&DynamodbChecksum],
    long_polling: false,
    replaces: &[(
        "botocore.discovery.block_endpoint_discovery_required_operations",
        Basis::Inert {
            acts_when: "an operation's endpoint discovery is required",
            refused_as: Refusal::Trait("endpointdiscovery"),
        },
    )],
};

/// dynamodb-streams, which the same two tables name; the checksum check is
/// dynamodb's only.
pub static DYNAMODB_STREAMS: Customization = Customization {
    id: "dynamodbstreams",
    scope: Scope::service("dynamodbstreams"),
    setting_defaults: &DYNAMODB_RETRIES,
    builtins: &[],
    interceptors: &[],
    retry_classifiers: &[],
    long_polling: false,
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
    interceptors: &[],
    retry_classifiers: &[&StsIdpRetry],
    long_polling: false,
    replaces: &[(
        "botocore.handlers.disable_signing",
        implemented("client::auth::select"),
    )],
};

#[derive(Debug)]
struct StsIdpRetry;

impl ClassifyRetry for StsIdpRetry {
    fn name(&self) -> &'static str {
        "sts-idp-communication-error"
    }
    fn classify_retry(
        &self,
        _: &mut InterceptorContext<'_>,
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

impl ClassifyRetry for DynamodbChecksum {
    fn name(&self) -> &'static str {
        "dynamodb-crc32"
    }
    /// `RetryDDBChecksumError.is_retryable`: the body's CRC32 differs from
    /// `x-amz-crc32`, read with Python's `int()`. That read is unguarded,
    /// so a header it cannot read raises its `ValueError` and fails the
    /// call; a negative or huge value simply mismatches.
    fn classify_retry(
        &self,
        _: &mut InterceptorContext<'_>,
        attempt: &Attempt<'_>,
    ) -> Option<RetryDecision> {
        let Attempt::Response { response, .. } = attempt else {
            return None;
        };
        let header = response.headers.get("x-amz-crc32")?;
        let matches = match model::pyrepr::int_from_text(header) {
            Err(text) => return Some(RetryDecision::Fail(HookError::Failed(text))),
            Ok(model::value::Value::Integer(expected)) => {
                i64::from(crc32(&response.body)) == expected
            }
            Ok(_) => false,
        };
        (!matches).then_some(RetryDecision::Retry)
    }
}

/// botocore's `_LONG_POLLING_OPERATIONS` (retries/standard.py), a table of
/// operations by hyphenized service id that its source says the
/// `aws.api#longPoll` trait will replace: once the retry quota refuses a
/// retry, the call still sleeps the delay it would have waited.
const fn long_polling(
    id: &'static str,
    service: &'static str,
    operation: &'static str,
) -> Customization {
    Customization {
        id,
        scope: Scope::operation(service, operation),
        setting_defaults: &[],
        builtins: &[],
        interceptors: &[],
        retry_classifiers: &[],
        long_polling: true,
        replaces: &[],
    }
}

pub static SQS_RECEIVE_MESSAGE: Customization =
    long_polling("sqs-long-poll", "sqs", "ReceiveMessage");
/// `sfn` in botocore's table, the hyphenized id of the `stepfunctions`
/// model.
pub static SFN_GET_ACTIVITY_TASK: Customization =
    long_polling("sfn-long-poll", "stepfunctions", "GetActivityTask");
pub static SWF_POLL_FOR_ACTIVITY_TASK: Customization =
    long_polling("swf-activity-long-poll", "swf", "PollForActivityTask");
pub static SWF_POLL_FOR_DECISION_TASK: Customization =
    long_polling("swf-decision-long-poll", "swf", "PollForDecisionTask");

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
