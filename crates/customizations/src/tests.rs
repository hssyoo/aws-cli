use super::*;
use client::endpoint::{EndpointParams, ParamValue};
use client::error::ServiceError;
use client::hooks::{CallContext, Hooks};
use client::http::{Headers, HttpResponse};
use client::retry::{Attempt, RetryDecision};
use model::shapes::Model;
use model::value::Value;
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

/// Every handler a customization says it replaces is a function botocore
/// defines where its dotted name says.
#[test]
fn replaced_handlers_exist_in_botocore() -> TestResult {
    for customization in TABLE {
        for name in customization.replaces {
            let (module, function) = name.rsplit_once('.').ok_or(*name)?;
            let path = module
                .strip_prefix("botocore.")
                .ok_or(*name)?
                .replace('.', "/");
            let source = std::fs::read_to_string(data().join(format!("{path}.py")))?;
            assert!(source.contains(&format!("def {function}(")), "{name}");
        }
    }
    Ok(())
}

/// The string literals of Rust source, skipping comments and character
/// literals; raw strings included.
fn literals(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                at = source[at..].find('\n').map_or(bytes.len(), |end| at + end);
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                at = source[at + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |end| at + end + 4);
            }
            b'\'' => at = skip_char(bytes, at),
            b'r' if matches!(bytes.get(at + 1), Some(b'"' | b'#')) && !word_before(bytes, at) => {
                let hashes = bytes[at + 1..].iter().take_while(|b| **b == b'#').count();
                let start = at + 2 + hashes;
                let close = format!("\"{}", "#".repeat(hashes));
                let end = source[start..]
                    .find(&close)
                    .map_or(bytes.len(), |end| start + end);
                found.push(source[start.min(end)..end].to_owned());
                at = end + close.len();
            }
            b'"' => {
                let mut end = at + 1;
                while end < bytes.len() && bytes[end] != b'"' {
                    end += if bytes[end] == b'\\' { 2 } else { 1 };
                }
                found.push(source[at + 1..end.min(bytes.len())].to_owned());
                at = end + 1;
            }
            _ => at += 1,
        }
    }
    found
}

fn word_before(bytes: &[u8], at: usize) -> bool {
    at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_')
}

/// Past a character literal, or past a lifetime's quote.
fn skip_char(bytes: &[u8], at: usize) -> usize {
    match (bytes.get(at + 1), bytes.get(at + 2), bytes.get(at + 3)) {
        (Some(b'\\'), _, _) => {
            at + bytes[at + 2..]
                .iter()
                .position(|b| *b == b'\'')
                .map_or(1, |end| end + 3)
        }
        (Some(_), Some(b'\''), _) => at + 3,
        _ => at + 1,
    }
}

/// Literals that are a service's directory name and mean something else:
/// each with why.
const NOT_SERVICES: [(&str, &str); 3] = [
    (
        "amp",
        "the XML entity for `&`, which the XML reader decodes",
    ),
    (
        "location",
        "a model trait key (`location`), not the Location service",
    ),
    ("ec2", "a wire protocol's name in botocore's priority list"),
];

fn service_names() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(data().join("data"))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    Ok(names)
}

/// The non-test source the rule covers: the core crates, and the router's
/// generic path.
fn core_sources() -> Result<Vec<std::path::PathBuf>, Box<dyn std::error::Error>> {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = vec![crates.join("cli-router/src/generic.rs")];
    let mut directories: Vec<std::path::PathBuf> = ["model", "client", "command", "format"]
        .iter()
        .map(|name| crates.join(name).join("src"))
        .filter(|path| path.is_dir())
        .collect();
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let path = entry?.path();
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
            if path.is_dir() {
                directories.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && !matches!(name.as_deref(), Some("tests.rs" | "testing.rs"))
            {
                files.push(path);
            }
        }
    }
    Ok(files)
}

/// The service names among `source`'s literals, but the known exceptions.
fn named_services<'a>(source: &str, names: &'a [String]) -> Vec<&'a str> {
    let literals = literals(source);
    names
        .iter()
        .map(String::as_str)
        .filter(|name| !NOT_SERVICES.iter().any(|(exception, _)| exception == name))
        .filter(|name| literals.iter().any(|literal| literal == name))
        .collect()
}

/// No service name in core (PLAN §2.2): every name is derived from the
/// models' directory, and none appears as a string literal in the
/// non-test source of the core crates or the router's generic path.
#[test]
fn no_core_source_names_a_service() -> TestResult {
    let names = service_names()?;
    assert!(names.len() > 400, "{} services", names.len());
    let mut found = Vec::new();
    let sources = core_sources()?;
    assert!(sources.len() > 20, "{sources:?}");
    for path in sources {
        let source = std::fs::read_to_string(&path)?;
        for name in named_services(&source, &names) {
            found.push(format!("{}: {name:?}", path.display()));
        }
    }
    assert!(found.is_empty(), "{found:#?}");
    Ok(())
}

/// The check finds a planted name, in an ordinary and in a raw string, and
/// not in a comment, a character literal or an exception.
#[test]
fn a_planted_service_name_is_found() -> TestResult {
    let names = service_names()?;
    let planted = r##"
        // "sts" in a comment is fine
        let quote = '"';
        let entity = "amp";
        let service = "dynamodb";
        let raw = r#"sts"#;
    "##;
    assert_eq!(named_services(planted, &names), ["dynamodb", "sts"]);
    assert!(named_services("let x = \"not-a-service\";", &names).is_empty());
    Ok(())
}
