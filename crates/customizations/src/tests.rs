use super::*;
use cli_core as _;
use client::error::ServiceError;
use client::hooks::{CallContext, Hooks};
use client::http::{Headers, HttpResponse};
use client::retry::{Attempt, RetryDecision};
use format as _;
use model::shapes::Model;
use model::value::Value;
use serde_json as _;
use std::time::Duration;
use timestamp as _;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn data() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../aws-cli/awscli/botocore")
}

/// `_SERVICE_MAX_ATTEMPTS` and `_SERVICE_BACKOFF_CONFIG`: dynamodb and
/// dynamodb-streams retry 4 times from a 25 ms base; any other service
/// keeps botocore's defaults.
#[test]
fn dynamodb_s_retry_defaults_and_no_one_else_s() {
    for service in ["dynamodb", "dynamodbstreams"] {
        let hooks = Hooks::for_service(&TABLE, service);
        assert_eq!(hooks.defaults().max_attempts(None), Some(4), "{service}");
        assert_eq!(hooks.defaults().max_attempts(Some(2)), Some(2), "{service}");
        assert_eq!(
            hooks.defaults().retry_base_delay(None),
            Some(Duration::from_millis(25)),
            "{service}"
        );
    }
    let sts = Hooks::for_service(&TABLE, "sts");
    assert_eq!(sts.defaults().max_attempts(None), None);
    assert_eq!(sts.defaults().retry_base_delay(None), None);
}

fn context_for<'a>(service: &'a str, model: &'a Model) -> Result<CallContext<'a>, &'static str> {
    let operation = model.operations().first().ok_or("no operation")?;
    Ok(CallContext::new(service, model, operation))
}

fn stub_model() -> Result<Model, Box<dyn std::error::Error>> {
    Ok(Model::from_json(
        br#"{"metadata": {}, "shapes": {},
        "operations": {"Op": {"name": "Op", "http": {"method": "POST", "requestUri": "/"}}}}"#,
    )?)
}

/// sts needs no interceptor for its global endpoint switch: core supplies
/// `AWS::STS::UseGlobalEndpoint` false for every client, as
/// `compute_endpoint_resolver_builtin_defaults` does, and no customization
/// sets a built-in.
#[test]
fn no_customization_sets_a_built_in() {
    for customization in TABLE {
        assert!(customization.builtins.is_empty(), "{}", customization.id);
    }
}

fn decision(
    service: &str,
    response: &HttpResponse,
    error: Option<&ServiceError>,
) -> TestResult<Option<RetryDecision>> {
    let model = stub_model()?;
    let mut cx = context_for(service, &model)?;
    let attempt = Attempt::Response { response, error };
    Ok(Hooks::for_service(&TABLE, service)
        .interceptors("Op")
        .find_map(|hook| hook.classify_retry(&mut cx, &attempt)))
}

/// `special.RetryIDPCommunicationError`: sts retries that code.
#[test]
fn sts_retries_an_idp_communication_error() -> TestResult {
    let response = HttpResponse {
        status: 400,
        headers: Headers::new(),
        body: Vec::new(),
    };
    let coded = |code: &str| {
        let mut error = ServiceError::new("AssumeRoleWithWebIdentity", 400);
        error
            .error
            .insert("Code".into(), Value::String(code.into()));
        error
    };
    let mut error = coded("IDPCommunicationError");
    assert_eq!(
        decision("sts", &response, Some(&error))?,
        Some(RetryDecision::Retry)
    );
    assert_eq!(decision("iam", &response, Some(&error))?, None);
    error = coded("AccessDenied");
    assert_eq!(decision("sts", &response, Some(&error))?, None);
    Ok(())
}

/// `special.RetryDDBChecksumError`: a dynamodb response whose
/// `x-amz-crc32` is not its body's CRC32 is retried.
#[test]
fn dynamodb_retries_a_body_whose_crc32_does_not_match() -> TestResult {
    let body = br#"{"TableNames":[]}"#.to_vec();
    let response = |crc: &str| HttpResponse {
        status: 200,
        headers: Headers::from_pairs([("x-amz-crc32", crc)]),
        body: body.clone(),
    };
    let actual = crc32(&body).to_string();
    assert_eq!(decision("dynamodb", &response(&actual), None)?, None);
    assert_eq!(
        decision("dynamodb", &response("1"), None)?,
        Some(RetryDecision::Retry)
    );
    assert_eq!(decision("dynamodbstreams", &response("1"), None)?, None);
    let unchecked = HttpResponse {
        status: 200,
        headers: Headers::new(),
        body,
    };
    assert_eq!(decision("dynamodb", &unchecked, None)?, None);
    Ok(())
}

/// `binascii.crc32`'s check value.
#[test]
fn crc32_is_binascii_s() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
}

/// Every handler a customization says it replaces is one botocore defines
/// where its dotted name says: a module file, then any classes, then the
/// function, as the pattern entry names handlers (`module.qualname`).
#[test]
fn replaced_handlers_exist_in_botocore() -> TestResult {
    for customization in TABLE {
        for name in customization.replaces {
            assert!(defined(name)?, "{name}");
        }
    }
    Ok(())
}

/// Whether botocore's source defines `name`: the longest dotted prefix
/// that is a module file, `class` lines for the parts between, and a
/// `def` for the last.
fn defined(name: &str) -> TestResult<bool> {
    let relative = name.strip_prefix("botocore.").ok_or(name)?;
    let parts: Vec<&str> = relative.split('.').collect();
    for split in (1..parts.len()).rev() {
        let file = data().join(format!("{}.py", parts[..split].join("/")));
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        let (function, classes) = parts[split..].split_last().ok_or(name)?;
        let lines: Vec<&str> = source.lines().map(str::trim_start).collect();
        let classes_found = classes.iter().all(|class| {
            lines.iter().any(|line| {
                line.starts_with(&format!("class {class}(")) || **line == format!("class {class}:")
            })
        });
        let function_found = lines
            .iter()
            .any(|line| line.starts_with(&format!("def {function}(")));
        return Ok(classes_found && function_found);
    }
    Ok(false)
}
