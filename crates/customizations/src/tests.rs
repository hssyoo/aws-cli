use super::*;
use client::endpoint::{EndpointParams, ParamValue};
use client::error::ServiceError;
use client::hooks::{CallContext, Hooks};
use client::http::{Headers, HttpResponse};
use client::retry::{Attempt, RetryDecision};
use model::shapes::Model;
use std::time::Duration;

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

/// `compute_endpoint_resolver_builtin_defaults`: `AWS::STS::UseGlobalEndpoint`
/// is false for sts.
#[test]
fn sts_never_uses_its_global_endpoint_unless_its_rules_do() -> TestResult {
    let model = stub_model()?;
    let mut cx = context_for("sts", &model)?;
    let mut params = EndpointParams::default();
    for hook in Hooks::for_service(&TABLE, "sts").interceptors("Op") {
        hook.endpoint_params(&mut cx, &mut params)?;
    }
    assert_eq!(
        params.get("UseGlobalEndpoint"),
        Some(&ParamValue::Boolean(false))
    );
    let mut other = EndpointParams::default();
    for hook in Hooks::for_service(&TABLE, "iam").interceptors("Op") {
        hook.endpoint_params(&mut cx, &mut other)?;
    }
    assert_eq!(other, EndpointParams::default());
    Ok(())
}

fn decision(service: &str, response: &HttpResponse, error: Option<&ServiceError>) -> TestResult<Option<RetryDecision>> {
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
    let mut error = ServiceError::new("AssumeRoleWithWebIdentity", 400);
    error.code = Some("IDPCommunicationError".into());
    assert_eq!(decision("sts", &response, Some(&error))?, Some(RetryDecision::Retry));
    assert_eq!(decision("iam", &response, Some(&error))?, None);
    error.code = Some("AccessDenied".into());
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
    assert_eq!(decision("dynamodb", &response("1"), None)?, Some(RetryDecision::Retry));
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

/// Every handler a customization says it replaces is a function botocore
/// defines where its dotted name says.
#[test]
fn replaced_handlers_exist_in_botocore() -> TestResult {
    for customization in TABLE {
        for name in customization.replaces {
            let (module, function) = name.rsplit_once('.').ok_or(*name)?;
            let path = module.strip_prefix("botocore.").ok_or(*name)?.replace('.', "/");
            let source = std::fs::read_to_string(data().join(format!("{path}.py")))?;
            assert!(source.contains(&format!("def {function}(")), "{name}");
        }
    }
    Ok(())
}
