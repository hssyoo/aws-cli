//! One sts and one dynamodb call, each made by the native client and by
//! botocore (`oracle.py`, in the toolchain's CPython) with the same
//! credentials, clock and uuids: the same URL, body, and headers in the
//! same order with the same values, the signature among them. Only the
//! `User-Agent` value differs, the two products naming themselves.
use cli_core::client_config::ClientSettings;
use cli_core::client_config::user_agent::{Host, Versions};
use client::call::{Client, ClientParts, UserAgentContext};
use client::credentials::{CredentialError, CredentialSource, SigningCredentials};
use client::endpoint::{EndpointConfig, EndpointData, EndpointInputs};
use client::http::{Headers, HttpRequest, HttpResponse};
use client::protocol::ParseOptions;
use client::testing::{FixedRuntime, InMemoryTransport};
use model::shapes::Model;
use model::value::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// 2015-08-30T12:36:00Z, the SigV4 suite's instant.
const EPOCH: i64 = 1_440_938_160;
const ACCESS_KEY: &str = "AKIDEXAMPLE";
const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
const TOKEN: &str = "token-1";

struct Case {
    /// The model directory and its API version.
    service: &'static str,
    version: &'static str,
    operation: &'static str,
    /// botocore's method name.
    method: &'static str,
    input: &'static str,
    region: &'static str,
    response: &'static str,
}

const CASES: [Case; 2] = [
    Case {
        service: "sts",
        version: "2011-06-15",
        operation: "GetCallerIdentity",
        method: "get_caller_identity",
        input: "{}",
        region: "us-east-1",
        response: "<GetCallerIdentityResponse><GetCallerIdentityResult><Account>1</Account>\
                   </GetCallerIdentityResult></GetCallerIdentityResponse>",
    },
    Case {
        service: "dynamodb",
        version: "2012-08-10",
        operation: "ListTables",
        method: "list_tables",
        input: r#"{"Limit": 5}"#,
        region: "us-west-2",
        response: r#"{"TableNames": []}"#,
    },
];

fn botocore_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../aws-cli/awscli/botocore")
}

#[derive(Debug)]
struct Keys;

impl CredentialSource for Keys {
    fn credentials(&self) -> Result<Option<SigningCredentials>, CredentialError> {
        Ok(Some(SigningCredentials {
            access_key_id: ACCESS_KEY.into(),
            secret_access_key: SECRET.into(),
            session_token: Some(TOKEN.into()),
            account_id: None,
        }))
    }
}

/// The native client's request for `case`.
fn native(case: &Case) -> TestResult<HttpRequest> {
    let data = botocore_root().join("data");
    let directory = data.join(case.service).join(case.version);
    let model = Model::from_json(&std::fs::read(directory.join("service-2.json"))?)?;
    let prefix = model
        .metadata()
        .endpoint_prefix
        .as_deref()
        .unwrap_or(case.service)
        .to_owned();
    let endpoints_json = std::fs::read(data.join("endpoints.json"))?;
    let endpoints = EndpointData::from_documents(
        std::fs::read(directory.join("endpoint-rule-set-1.json"))?,
        std::fs::read(data.join("partitions.json"))?,
        Some((&endpoints_json, &prefix)),
    )?;
    let transport = Arc::new(InMemoryTransport::new());
    transport.respond(HttpResponse {
        status: 200,
        headers: Headers::new(),
        body: case.response.as_bytes().to_vec(),
    });
    let client = Client::new(ClientParts {
        service: case.service.into(),
        model: Arc::new(model),
        endpoints,
        endpoint: EndpointConfig::new(&EndpointInputs {
            region: Some(case.region),
            signing: true,
            ..EndpointInputs::default()
        }),
        settings: ClientSettings::new(format!("{}.{}", case.service, case.method)),
        credentials: Arc::new(Keys),
        transport: transport.clone(),
        runtime: Arc::new(FixedRuntime::new(
            timestamp::Timestamp::from_unix_seconds(EPOCH).ok_or("epoch")?,
            0.0,
        )),
        customizations: &customizations::TABLE,
        parse: ParseOptions::BOTOCORE,
        user_agent: UserAgentContext {
            host: Host {
                system: "Linux".into(),
                release: "6.1".into(),
                machine: "x86_64".into(),
            },
            distribution: None,
            versions: Versions {
                cli: "2.0.0",
                crt: "0.1.0",
                rust: "1.96.0",
            },
        },
    })?;
    client.call(case.operation, Value::from_json(case.input.as_bytes())?)?;
    let sent = transport.sent();
    Ok(sent.first().ok_or("nothing sent")?.0.clone())
}

/// botocore's requests for `cases`, as `oracle.py` captures them.
fn botocore(cases: &[Case]) -> TestResult<Vec<Value>> {
    let prefix = std::env::var_os("AWS_NATIVE_CPYTHON_PREFIX")
        .ok_or("AWS_NATIVE_CPYTHON_PREFIX must name the toolchain CPython")?;
    let oracle = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/signature_parity/oracle.py");
    let input: Vec<Value> = cases
        .iter()
        .map(|case| -> TestResult<Value> {
            let mut keys = Map::new();
            keys.insert("access_key_id".into(), Value::String(ACCESS_KEY.into()));
            keys.insert("secret_access_key".into(), Value::String(SECRET.into()));
            keys.insert("session_token".into(), Value::String(TOKEN.into()));
            let mut map = Map::new();
            map.insert("service".into(), Value::String(case.service.into()));
            map.insert("method".into(), Value::String(case.method.into()));
            map.insert("params".into(), Value::from_json(case.input.as_bytes())?);
            map.insert("region".into(), Value::String(case.region.into()));
            map.insert("epoch".into(), Value::Integer(EPOCH));
            map.insert("credentials".into(), Value::Map(keys));
            Ok(Value::Map(map))
        })
        .collect::<Result<_, _>>()?;
    let mut child = std::process::Command::new(PathBuf::from(prefix).join("bin/python3"))
        .arg("-I")
        .arg(oracle)
        .arg(
            botocore_root()
                .parent()
                .and_then(Path::parent)
                .ok_or("parent")?,
        )
        .env_clear()
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let input = model::pyjson::dumps(
        &Value::List(input),
        &model::pyjson::Options::compact(),
        None,
    )?;
    std::io::Write::write_all(&mut child.stdin.take().ok_or("stdin")?, input.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    match Value::from_json(&output.stdout)? {
        Value::List(results) => Ok(results),
        other => Err(format!("{other:?}").into()),
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    match value {
        Value::Map(map) => match map.get(key)? {
            Value::String(text) => Some(text),
            _ => None,
        },
        _ => None,
    }
}

/// A request as `[name, value]` pairs, `User-Agent`'s value blanked.
fn comparable<'a>(headers: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<(String, String)> {
    headers
        .map(|(name, value)| {
            let value = if name.eq_ignore_ascii_case("User-Agent") {
                ""
            } else {
                value
            };
            (name.to_owned(), value.to_owned())
        })
        .collect()
}

fn oracle_headers(request: &Value) -> TestResult<Vec<(String, String)>> {
    let Value::Map(map) = request else {
        return Err("no request".into());
    };
    let Some(Value::List(headers)) = map.get("headers") else {
        return Err("no headers".into());
    };
    let pairs = headers
        .iter()
        .map(|pair| match pair {
            Value::List(pair) => match pair.as_slice() {
                [Value::String(name), Value::String(value)] => Ok((&**name, &**value)),
                _ => Err("header pair"),
            },
            _ => Err("header pair"),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(comparable(pairs.into_iter()))
}

#[test]
fn sts_and_dynamodb_requests_match_botocore_s_signature_and_all() -> TestResult {
    let expected = botocore(&CASES)?;
    assert_eq!(expected.len(), CASES.len());
    for (case, botocore) in CASES.iter().zip(&expected) {
        let request = native(case)?;
        let service = case.service;
        assert_eq!(
            Some(request.method.as_str()),
            text(botocore, "method"),
            "{service}"
        );
        assert_eq!(
            Some(request.url.to_string().as_str()),
            text(botocore, "url"),
            "{service}"
        );
        assert_eq!(
            Some(String::from_utf8_lossy(&request.body).as_ref()),
            text(botocore, "body"),
            "{service}"
        );
        assert_eq!(
            comparable(request.headers.iter()),
            oracle_headers(botocore)?,
            "{service}"
        );
    }
    Ok(())
}
