//! Each case of `cases.json` against the Python route's stderr and exit
//! status, which `capture.py` recorded in `fixtures/` by running the built
//! binary with `AWS_CLI_NATIVE_ROUTER=python` against a loopback stub:
//!
//! ```text
//! python3 crates/service-errors/tests/capture.py \
//!     target/aarch64-apple-darwin/release-safe/aws
//! ```
//!
//! Natively, each case is a real call: the service's model, rule set,
//! validation and protocol, on the in-memory transport, which answers with
//! the stub's response or fails as the case says. Its error goes through
//! [`service_errors::call_failure`] and `cli_core`'s renderer.
use cli_core::cli_error::{CommandFailure, ErrorFormat, exit_status, render_command_failure};
use cli_core::client_config::user_agent::{Host, Versions};
use cli_core::client_config::{ClientSettings, Resolved, Source};
use client::call::{Client, ClientParts, UserAgentContext};
use client::credentials::{CredentialError, CredentialSource, SigningCredentials};
use client::endpoint::{EndpointConfig, EndpointData, EndpointInputs};
use client::error::CallError;
use client::http::{Headers, HttpResponse};
use client::testing::{FixedRuntime, InMemoryTransport};
use client::transport::TransportError;
use model::shapes::Model;
use model::value::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The port the native calls name, written `{port}` in the fixtures.
const PORT: &str = "4567";

fn here() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../aws-cli/awscli/botocore/data")
}

#[derive(Debug)]
struct Keys(bool);

impl CredentialSource for Keys {
    fn credentials(&self) -> Result<Option<SigningCredentials>, CredentialError> {
        Ok(self.0.then(|| SigningCredentials {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            session_token: None,
            account_id: None,
        }))
    }
}

fn text<'a>(case: &'a Value, name: &str) -> Option<&'a str> {
    case.get(name)?.as_str()
}

fn strings(case: &Value, name: &str) -> Vec<String> {
    let Some(Value::List(items)) = case.get(name) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// The model directory of `service`: its one API version.
fn service_directory(service: &str) -> TestResult<PathBuf> {
    let mut versions: Vec<PathBuf> = std::fs::read_dir(data().join(service))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    versions.sort();
    Ok(versions.pop().ok_or(format!("no model for {service}"))?)
}

/// The stub's response, as the in-memory transport answers.
fn response(stub: &Value) -> TestResult<HttpResponse> {
    let status = match stub.get("status") {
        Some(Value::Integer(status)) => u16::try_from(*status)?,
        _ => return Err("stub status".into()),
    };
    let headers: Vec<(String, String)> = match stub.get("headers") {
        Some(Value::List(pairs)) => pairs
            .iter()
            .filter_map(|pair| match pair {
                Value::List(pair) => match pair.as_slice() {
                    [Value::String(name), Value::String(value)] => {
                        Some((name.to_string(), value.to_string()))
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(HttpResponse {
        status,
        headers: Headers::from_pairs(headers.iter().map(|(n, v)| (n.as_str(), v.as_str()))),
        body: text(stub, "body").unwrap_or("").as_bytes().to_vec(),
    })
}

fn transport_failure(case: &Value, kind: &str) -> TestResult<TransportError> {
    Ok(match kind {
        "proxy" => TransportError::Proxy {
            url: case
                .get("env")
                .and_then(|env| text(env, "HTTPS_PROXY"))
                .ok_or("proxy")?
                .replace("{port}", PORT),
        },
        "connect" => TransportError::Connect,
        "connect_timeout" => TransportError::ConnectTimeout,
        "read_timeout" => TransportError::ReadTimeout,
        "connection_closed" => TransportError::ConnectionClosed,
        // The CRT's message for AWS_IO_TLS_ERROR_NEGOTIATION_FAILURE, which
        // the transport reports (`client::transport::crt`).
        "tls" => TransportError::Tls {
            reason: "TLS (SSL) negotiation failed".into(),
        },
        other => return Err(format!("transport {other}").into()),
    })
}

/// `service`'s model and endpoint data, as the payload holds them.
fn service_data(service: &str) -> TestResult<(Arc<Model>, EndpointData)> {
    let directory = service_directory(service)?;
    let model = Arc::new(Model::from_json(&std::fs::read(
        directory.join("service-2.json"),
    )?)?);
    let prefix = model
        .metadata()
        .endpoint_prefix
        .as_deref()
        .unwrap_or(service)
        .to_owned();
    let endpoints_json = std::fs::read(data().join("endpoints.json"))?;
    let endpoints = EndpointData::from_documents(
        std::fs::read(directory.join("endpoint-rule-set-1.json"))?,
        std::fs::read(data().join("partitions.json"))?,
        Some((&endpoints_json, &prefix)),
    )?;
    Ok((model, endpoints))
}

/// The transport for `attempts` attempts: the stub's response each time,
/// or the case's failure.
fn transport_for(case: &Value, attempts: u64) -> TestResult<Arc<InMemoryTransport>> {
    let transport = Arc::new(InMemoryTransport::new());
    for _ in 0..attempts {
        // A case that fails in transport fails natively as it did on the
        // Python route, whatever the stub did to cause it.
        if let Some(kind) = text(case, "transport") {
            transport.fail(transport_failure(case, kind)?);
        } else if let Some(stub) = case.get("stub") {
            transport.respond(response(stub)?);
        }
    }
    Ok(transport)
}

/// The case's call, natively: its error and its service's model.
fn native_error(case: &Value) -> TestResult<(CallError, Arc<Model>)> {
    let service = text(case, "service").ok_or("service")?;
    let (model, endpoints) = service_data(service)?;
    let argv = strings(case, "argv");
    let endpoint_url = argv
        .iter()
        .position(|arg| arg == "--endpoint-url")
        .and_then(|at| argv.get(at + 1))
        .map(|url| url.replace("{port}", PORT));
    let region = match case.get("region") {
        Some(Value::Null) => None,
        Some(Value::String(region)) => Some(region.to_string()),
        _ => Some("us-west-2".to_owned()),
    };
    // The capture sets 1 unless the case says otherwise; `null` unsets
    // it, leaving botocore's default, 3.
    let attempts: u64 = match case.get("env").and_then(|env| env.get("AWS_MAX_ATTEMPTS")) {
        Some(Value::String(attempts)) => attempts.parse()?,
        Some(_) => 3,
        None => 1,
    };
    let mut settings = ClientSettings::new(format!("{service}.test"));
    settings.max_attempts = Some(Resolved::new(attempts, Source::Env("AWS_MAX_ATTEMPTS")));
    let transport = transport_for(case, attempts)?;
    let parts = ClientParts {
        service: service.into(),
        model: model.clone(),
        endpoints,
        endpoint: EndpointConfig::new(&EndpointInputs {
            region: region.as_deref(),
            endpoint_url: endpoint_url.as_deref(),
            use_fips: case.get("fips").map(|_| true),
            use_dual_stack: case.get("dual_stack").map(|_| true),
            signing: true,
        }),
        settings,
        credentials: Arc::new(Keys(case.get("no_credentials").is_none())),
        transport,
        runtime: Arc::new(FixedRuntime::new(
            timestamp::Timestamp::from_unix_seconds(1_440_938_160).ok_or("epoch")?,
            0.0,
        )),
        customizations: &[],
        parse: client::protocol::ParseHooks::botocore(),
        user_agent: UserAgentContext {
            host: Host::default(),
            distribution: None,
            versions: Versions {
                cli: "2.0.0",
                crt: "0.1.0",
                rust: "1.96.0",
            },
            extras: client::useragent::SessionExtras {
                installer: "other",
                prompt: "off",
                command: format!("{service}.test"),
            },
        },
    };
    let input = case
        .get("input")
        .cloned()
        .unwrap_or(Value::Map(model::value::Map::new()));
    let operation = text(case, "operation").ok_or("operation")?;
    let error = match Client::new(parts) {
        Err(error) => error,
        Ok(client) => match client.call(operation, input) {
            Err(error) => error,
            Ok(output) => return Err(format!("the call succeeded: {output:?}").into()),
        },
    };
    Ok((error, model))
}

/// The case's failure, natively: a real call's, or for the cases without a
/// service, the pager's or the credential chain's.
fn native_failure(case: &Value) -> TestResult<CommandFailure> {
    if case.get("tty_stdout").is_some() {
        let pager = case
            .get("env")
            .and_then(|env| text(env, "AWS_PAGER"))
            .ok_or("pager")?;
        let interrupt = cli_core::interrupt::State::new();
        let error = format::pager::run(&[pager.to_owned()], &[], b"{}", &interrupt)
            .err()
            .ok_or("the pager started")?;
        return Ok(service_errors::pager_failure(&error));
    }
    if text(case, "service").is_none() {
        // The process provider's failure as the credentials crate words
        // it, for the helper's stderr (`credentials::process`), reaching
        // the client's call as the model client's chain hands it over.
        let error = credentials::LoadError::CredentialRetrieval {
            provider: "custom-process",
            error_msg: "boom\n".into(),
        };
        let (model, _) = service_data("sts")?;
        let error = CallError::Credentials(error.to_string());
        return Ok(service_errors::call_failure(&error, &model));
    }
    let (error, model) = native_error(case)?;
    Ok(service_errors::call_failure(&error, &model))
}

fn format_of(case: &Value) -> ErrorFormat {
    let argv = strings(case, "argv");
    match argv.iter().position(|arg| arg == "--cli-error-format") {
        Some(at) if argv.get(at + 1).map(String::as_str) == Some("legacy") => ErrorFormat::Legacy,
        _ => ErrorFormat::Enhanced,
    }
}

#[test]
fn every_case_renders_as_the_python_route_does() -> TestResult {
    let Value::List(cases) = Value::from_json(&std::fs::read(here().join("cases.json"))?)? else {
        return Err("cases.json".into());
    };
    let mut mismatches = Vec::new();
    for case in &cases {
        let name = text(case, "name").ok_or("name")?;
        let fixture = here().join("fixtures").join(name);
        let expected = std::fs::read(fixture.with_extension("stderr"))?;
        let status: u8 = std::fs::read_to_string(fixture.with_extension("status"))?
            .trim()
            .parse()?;
        let failure = native_failure(case)?;
        let mut rendered = Vec::new();
        render_command_failure(&mut rendered, &failure, format_of(case))?;
        let rendered = String::from_utf8_lossy(&rendered)
            .replace(&format!("127.0.0.1:{PORT}"), "127.0.0.1:{port}");
        let expected = String::from_utf8_lossy(&expected);
        // A text whose end is the TLS library's matches up to that point.
        let matched = match text(case, "gap") {
            Some(prefix) => {
                let prefix = format!("\naws: [ERROR]: {prefix}");
                rendered.starts_with(&prefix) && expected.starts_with(&prefix)
            }
            None => rendered == expected,
        };
        if !matched || exit_status(failure.category) != status {
            mismatches.push(format!(
                "{name}: {} {rendered:?}\n  python {status} {expected:?}",
                exit_status(failure.category)
            ));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    Ok(())
}
