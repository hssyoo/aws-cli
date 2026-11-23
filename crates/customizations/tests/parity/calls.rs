//! Layer B: a case's call through `client::call::Client` as the CLI builds
//! it, with its customizations, against an in-memory transport answering
//! every attempt with the case's canned response, a runtime with the
//! oracle's pinned clock and uuids numbered from 1, and the case's keys,
//! as `oracle.py`'s `layer_b` makes it through the CLI's own session.
use super::{Result, decode, repository, response};
use cli_core::client_config::user_agent::{Host, Versions};
use cli_core::client_config::{ClientSettings, Resolved, Source as SettingSource};
use client::call::{Client, ClientParts, UserAgentContext};
use client::credentials::{CredentialError, CredentialSource, SigningCredentials};
use client::endpoint::{EndpointConfig, EndpointData, EndpointInputs};
use client::error::CallError;
use client::http::{HttpRequest, HttpResponse};
use client::protocol::ParseOptions;
use client::testing::{FixedRuntime, InMemoryTransport};
use model::shapes::Model;
use model::source::{DirSource, Kind, LoadError, Source};
use model::value::Value;
use serde_json::{Value as Json, json};
use std::sync::Arc;
use timestamp::Timestamp;

/// `oracle.PINNED`, 2026-11-22T21:24:03Z.
const PINNED: i64 = 1_795_382_643;

/// botocore's `Loader` over the oracle's search path: the fake models
/// first for a fake case, then botocore's data; services and versions are
/// every location's, a file the first location's that has it.
#[derive(Debug)]
struct Locations(Vec<DirSource>);

impl Source for Locations {
    fn services(&self, kind: Kind) -> std::result::Result<Vec<String>, LoadError> {
        let mut services = Vec::new();
        for location in &self.0 {
            services.extend(location.services(kind)?);
        }
        services.sort();
        services.dedup();
        Ok(services)
    }
    fn versions(&self, service: &str, kind: Kind) -> std::result::Result<Vec<String>, LoadError> {
        let mut versions = Vec::new();
        for location in &self.0 {
            versions.extend(location.versions(service, kind)?);
        }
        versions.sort();
        versions.dedup();
        Ok(versions)
    }
    fn read(&self, name: &str) -> std::result::Result<Option<Vec<u8>>, LoadError> {
        for location in &self.0 {
            if let Some(bytes) = location.read(name)? {
                return Ok(Some(bytes));
            }
        }
        Ok(None)
    }
}

fn locations(case: &Json) -> Locations {
    let botocore = DirSource::new(repository().join("aws-cli/awscli/botocore/data"));
    if case["models"] == "fake" {
        Locations(vec![
            DirSource::new(repository().join("tests/models")),
            botocore,
        ])
    } else {
        Locations(vec![botocore])
    }
}

/// The case's keys, as the oracle puts them in the environment.
#[derive(Debug)]
struct Keys(SigningCredentials);

impl CredentialSource for Keys {
    fn credentials(&self) -> std::result::Result<Option<SigningCredentials>, CredentialError> {
        Ok(Some(self.0.clone()))
    }
}

fn keys(settings: &Json) -> Keys {
    let text = |key: &str, default: &str| settings[key].as_str().unwrap_or(default).to_owned();
    Keys(SigningCredentials {
        access_key_id: text("access_key", "AKIDEXAMPLE"),
        secret_access_key: text("secret_key", "secret"),
        session_token: settings["session_token"].as_str().map(str::to_owned),
        account_id: None,
    })
}

fn user_agent() -> UserAgentContext {
    UserAgentContext {
        host: Host {
            system: "Darwin".into(),
            release: "27.2.0".into(),
            machine: "arm64".into(),
        },
        distribution: None,
        versions: Versions {
            cli: "2.40.0",
            crt: "0.38.1",
            rust: "1.96.0",
        },
    }
}

/// The client `oracle.py`'s `layer_b` makes for `case`, and its transport.
fn client(case: &Json, attempts: u64) -> Result<(Client, Arc<InMemoryTransport>)> {
    let service = case["service"].as_str().ok_or("service")?;
    let settings = &case["settings"];
    let source = locations(case);
    let model = Model::load(&source, service)?;
    let endpoints = EndpointData::load(&source, service, model.metadata())?;
    let canned = response(case).unwrap_or_else(|| HttpResponse {
        status: 200,
        ..HttpResponse::default()
    });
    let transport = Arc::new(InMemoryTransport::new());
    for _ in 0..attempts {
        transport.respond(canned.clone());
    }
    let endpoint_url = settings["endpoint_url"].as_str();
    let mut client_settings = ClientSettings::new(format!("{service}.call"));
    client_settings.max_attempts = Some(Resolved::new(attempts, SettingSource::Default));
    client_settings.endpoint_override = endpoint_url.is_some();
    // The oracle's keys are in the environment.
    client_settings.credential_source = cli_core::command::CredentialSource::Environment;
    let client = Client::new(ClientParts {
        service: service.to_owned(),
        model: Arc::new(model),
        endpoints,
        endpoint: EndpointConfig::new(&EndpointInputs {
            region: Some(settings["region"].as_str().unwrap_or("us-west-2")),
            use_fips: Some(settings["fips"].as_bool().unwrap_or(false)),
            use_dual_stack: Some(settings["dualstack"].as_bool().unwrap_or(false)),
            endpoint_url,
            signing: true,
        }),
        settings: client_settings,
        credentials: Arc::new(keys(settings)),
        transport: transport.clone(),
        runtime: Arc::new(FixedRuntime::new(
            Timestamp::from_unix_seconds(PINNED).ok_or("pinned instant")?,
            0.0,
        )),
        customizations: &customizations::TABLE,
        parse: ParseOptions::BOTOCORE,
        user_agent: user_agent(),
    })?;
    Ok((client, transport))
}

/// A request as the oracle records it, `User-Agent` reduced to its `m/`
/// feature codes as a set (PLAN §2.1, decision 3): the rest of the value
/// names the product and its host.
fn request_json(request: &HttpRequest) -> Json {
    let headers: Vec<Json> = request
        .headers
        .iter()
        .map(|(name, value)| json!([name, comparable_header(name, value)]))
        .collect();
    json!({
        "method": request.method.as_str(),
        "url": request.url.to_string(),
        "headers": headers,
        "body": model::base64::encode(&request.body),
    })
}

fn comparable_header(name: &str, value: &str) -> String {
    if !name.eq_ignore_ascii_case("User-Agent") {
        return value.to_owned();
    }
    let mut codes: Vec<&str> = value
        .split(' ')
        .filter_map(|token| token.strip_prefix("m/"))
        .flat_map(|codes| codes.split(','))
        .collect();
    codes.sort_unstable();
    format!("m/{}", codes.join(","))
}

/// The oracle's capture with its `User-Agent` values made comparable.
pub fn expected(b: &Json) -> Json {
    let mut b = b.clone();
    for request in b["requests"].as_array_mut().into_iter().flatten() {
        for header in request["headers"].as_array_mut().into_iter().flatten() {
            if let (Some(name), Some(value)) = (header[0].as_str(), header[1].as_str()) {
                header[1] = Json::String(comparable_header(name, value));
            }
        }
    }
    b
}

/// The exception botocore raises for `error`: a modelled error's class is
/// its shape's name (`ClientExceptionsFactory`).
fn error_json(model: &Model, error: &CallError) -> Json {
    let class = match error {
        CallError::Validation(_) => "ParamValidationError".to_owned(),
        CallError::Parse(_) => "ResponseParserError".to_owned(),
        CallError::Service(service) => service
            .code()
            .and_then(|code| model.shape_for_error_code(code))
            .map_or("ClientError".to_owned(), |id| {
                model.shape(id).name.to_string()
            }),
        other => format!("{other:?}"),
    };
    json!({ "class": class, "text": error.to_string() })
}

/// Layer B for one case, as `oracle.py` writes it.
pub fn layer_b(case: &Json) -> Result<Json> {
    let attempts = case["settings"]["max_attempts"].as_u64().unwrap_or(3);
    let (client, transport) = client(case, attempts)?;
    let operation = case["operation"].as_str().ok_or("operation")?;
    let input = decode(Value::from_serde(&case["input"]))?;
    let outcome = client.call(operation, input);
    let requests: Vec<Json> = transport
        .sent()
        .iter()
        .map(|(request, _)| request_json(request))
        .collect();
    let mut vector = json!({ "requests": requests });
    match outcome {
        Ok(output) => {
            let mut value = output.value;
            if let Value::Map(map) = &mut value
                && let Some(Value::Map(metadata)) = map.get_mut("ResponseMetadata")
            {
                // `Endpoint._send_request` adds it to the parsed response.
                let attempts = i64::from(output.retry_attempts);
                metadata.insert("RetryAttempts".into(), Value::Integer(attempts));
            }
            vector["parsed"] = Json::String(format::json::dump(&value));
        }
        Err(error) => {
            let model = Model::load(&locations(case), case["service"].as_str().unwrap_or(""))?;
            vector["error"] = error_json(&model, &error);
        }
    }
    Ok(vector)
}
