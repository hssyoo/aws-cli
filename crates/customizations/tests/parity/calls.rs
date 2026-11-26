//! Layer B: a case's call through `client::call::Client` as the CLI builds
//! it, with its customizations, against an in-memory transport answering
//! every attempt with the case's canned response, a runtime with the
//! oracle's pinned clock and uuids numbered from 1, and the case's keys,
//! as `oracle.py`'s `layer_b` makes it through the CLI's own session.
use super::{Result, canned, decode, repository, response};
use cli_core::client_config::user_agent::{Host, Versions};
use cli_core::client_config::{ClientSettings, Resolved, Source as SettingSource};
use client::error::CallError;
use client::http::{HttpRequest, HttpResponse};
use client::testing::facade::{
    self, Client, ClientCase, Credentials, EndpointCase, InMemoryTransport, Keys, Model,
    PaginationCase, UserAgentCase,
};
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

/// The case's keys, as the oracle puts them in the environment. The oracle
/// supplies the keys in the environment, so botocore's `EnvProvider`
/// answers.
fn keys(settings: &Json) -> Credentials {
    let text = |key: &str, default: &str| settings[key].as_str().unwrap_or(default).to_owned();
    Credentials::Static {
        keys: Keys {
            access_key_id: text("access_key", "AKIDEXAMPLE"),
            secret_access_key: text("secret_key", "secret"),
            session_token: settings["session_token"].as_str().map(str::to_owned),
            account_id: None,
        },
        method: Some("env"),
    }
}

fn user_agent() -> UserAgentCase {
    UserAgentCase {
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
        // The comparison reads only the `m/` feature codes.
        installer: "other",
        prompt: "off",
        command: String::new(),
    }
}

/// More requests than any single call's attempts, or a paginated case's
/// pages and their attempts, make.
const RESPONSES: usize = 30;

/// The client `oracle.py`'s `layer_b` makes for `case`, and its transport.
fn client(case: &Json) -> Result<(Client, Arc<InMemoryTransport>)> {
    let service = case["service"].as_str().ok_or("service")?;
    let settings = &case["settings"];
    let source = locations(case);
    let model = facade::load_model(&source, service)?;
    let endpoints = facade::endpoint_data(&source, service, &model)?;
    // One canned response per request, in order, the last answering every
    // request after it, as the oracle's capture answers.
    let answers: Vec<HttpResponse> = match case["responses"].as_array() {
        Some(responses) if !responses.is_empty() => responses.iter().map(canned).collect(),
        _ => vec![response(case).unwrap_or_else(|| HttpResponse {
            status: 200,
            ..HttpResponse::default()
        })],
    };
    let transport = Arc::new(InMemoryTransport::new());
    for index in 0..RESPONSES.max(answers.len()) {
        let answer = answers.get(index).or(answers.last());
        transport.respond(answer.cloned().unwrap_or_default());
    }
    let endpoint_url = settings["endpoint_url"].as_str();
    let mut client_settings = ClientSettings::new(format!("{service}.call"));
    client_settings.max_attempts = settings["max_attempts"]
        .as_u64()
        .map(|attempts| Resolved::new(attempts, SettingSource::Profile("max_attempts")));
    client_settings.endpoint_override = endpoint_url.is_some();
    // The oracle's keys are in the environment.
    client_settings.credential_source = cli_core::command::CredentialSource::Environment;
    let client = facade::client_case(ClientCase {
        service: service.to_owned(),
        model,
        endpoints,
        endpoint: EndpointCase {
            region: Some(
                settings["region"]
                    .as_str()
                    .unwrap_or("us-west-2")
                    .to_owned(),
            ),
            use_fips: Some(settings["fips"].as_bool().unwrap_or(false)),
            use_dual_stack: Some(settings["dualstack"].as_bool().unwrap_or(false)),
            endpoint_url: endpoint_url.map(str::to_owned),
        },
        settings: client_settings,
        credentials: keys(settings),
        transport: transport.clone(),
        now: Timestamp::from_unix_seconds(PINNED).ok_or("pinned instant")?,
        random: 0.0,
        customizations: &customizations::TABLE,
        user_agent: user_agent(),
    })?;
    Ok((client, transport))
}

/// A request as the oracle records it, `User-Agent` reduced to its `m/`
/// feature codes as a set, since botocore joins its set in no fixed order
/// (useragent.py `UserAgentString._build_feature_metadata`): the rest of
/// the value names the product and its host.
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
        "body": pytext::base64::encode(&request.body),
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
            .and_then(|code| facade::error_shape_name(model, code))
            .unwrap_or_else(|| "ClientError".to_owned()),
        other => format!("{other:?}"),
    };
    json!({ "class": class, "text": error.to_string() })
}

/// Layer B for one case, as `oracle.py` writes it.
pub fn layer_b(case: &Json) -> Result<Json> {
    let (client, transport) = client(case)?;
    let operation = case["operation"].as_str().ok_or("operation")?;
    let input = decode(Value::from_serde(&case["input"]))?;
    let outcome = match case.get("paginate") {
        Some(pagination) => paginated(case, &client, operation, input, pagination)?,
        None => facade::call_case(&client, operation, input).map(|(value, retry_attempts)| {
            let mut value = value;
            if let Value::Map(map) = &mut value
                && let Some(Value::Map(metadata)) = map.get_mut("ResponseMetadata")
            {
                // `Endpoint._send_request` adds it to the parsed response.
                let attempts = i64::from(retry_attempts);
                metadata.insert("RetryAttempts".into(), Value::Integer(attempts));
            }
            value
        }),
    };
    let requests: Vec<Json> = transport
        .sent()
        .iter()
        .map(|(request, _)| request_json(request))
        .collect();
    let mut vector = json!({ "requests": requests });
    match outcome {
        Ok(value) => {
            vector["parsed"] = Json::String(super::dumped(&value));
        }
        Err(error) => {
            let service = case["service"].as_str().unwrap_or("");
            let model = facade::load_model(&locations(case), service)?;
            vector["error"] = error_json(&model, &error);
        }
    }
    Ok(vector)
}

/// A case with `paginate`: every page of `operation` through
/// `client::paginate::PageIterator` with the operation's paginator entry,
/// then `build_full_result`, as the oracle's paginator gives it.
fn paginated(
    case: &Json,
    client: &Client,
    operation: &str,
    input: Value,
    pagination: &Json,
) -> Result<std::result::Result<Value, CallError>> {
    let service = case["service"].as_str().ok_or("service")?;
    let option = |key: &str| pagination.get(key).map(Value::from_serde);
    let pagination = PaginationCase {
        max_items: option("MaxItems"),
        page_size: option("PageSize"),
        starting_token: pagination["StartingToken"].as_str().map(str::to_owned),
    };
    Ok(facade::paginate_case(
        client,
        &locations(case),
        service,
        operation,
        input,
        pagination,
    )?)
}
