//! The frozen test façade: everything the frozen harnesses (the protocol
//! suite, parity layers A and B, the census, `service-errors`'
//! `python_route`, [`super::fuzz`] and the fuzz targets) read from the
//! product, as data.
//!
//! The signatures here are frozen: the harnesses that call them may not
//! change, so a product change rewrites a body, never a harness. Every
//! function produces data; every comparison, tally and assertion stays in
//! the harness. A type a harness names is re-exported here, so a moved type
//! changes a re-export, not the harness. `tests/facade_api.rs` in
//! `customizations` pins every signature.
use crate::admission::{Unsupported, static_refusal};
use crate::credentials::{CredentialError, CredentialSource, SigningCredentials};
use crate::endpoint::{EndpointConfig, EndpointInputs};
use crate::protocol::{ClientProtocol, IMPLEMENTED};
use crate::useragent::{SessionExtras, UserAgentContext};
use model::paginators::{Paginators, Refused};
use model::source::{Kind, LoadError, Source};
use model::validate::{Validation, validate_input};
use model::value::{Map, Value};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

pub use crate::call::Client;
pub use crate::endpoint::EndpointData;
pub use crate::error::{CallError, ParseError, SerializeError, ServiceError};
pub use crate::hooks::Customization;
pub use crate::http::{Headers, HttpRequest, HttpResponse};
pub use crate::protocol::{BlobHandler, ParseOptions, Serialized, TimestampHandler};
pub use crate::testing::{FixedRuntime, InMemoryTransport};
pub use cli_core::client_config::ClientSettings;
pub use cli_core::client_config::user_agent::{Distribution, Host, Versions};
pub use model::shapes::Model;
pub use timestamp::Timestamp;

pub mod hooks;

/// `&dyn Source` as the loaders' `impl Source`.
struct Dyn<'a>(&'a dyn Source);

impl Source for Dyn<'_> {
    fn services(&self, kind: Kind) -> Result<Vec<String>, LoadError> {
        self.0.services(kind)
    }
    fn versions(&self, service: &str, kind: Kind) -> Result<Vec<String>, LoadError> {
        self.0.versions(service, kind)
    }
    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, LoadError> {
        self.0.read(name)
    }
    fn contains(&self, name: &str) -> Result<bool, LoadError> {
        self.0.contains(name)
    }
}

// ---------------------------------------------------------------- models

/// The service's latest model in `source`, extras merged.
///
/// # Errors
/// The loader's error, as text.
pub fn load_model(source: &dyn Source, service: &str) -> Result<Arc<Model>, String> {
    Model::load(&Dyn(source), service)
        .map(Arc::new)
        .map_err(|error| error.to_string())
}

/// A model from `service-2.json` bytes.
///
/// # Errors
/// The model reader's error, as text.
pub fn model_from_json(bytes: &[u8]) -> Result<Arc<Model>, String> {
    Model::from_json(bytes)
        .map(Arc::new)
        .map_err(|error| error.to_string())
}

/// The operations' names, in model order.
pub fn operation_names(model: &Model) -> Vec<String> {
    model
        .schema()
        .service()
        .operations()
        .map(|operation| operation.name().to_owned())
        .collect()
}

/// The model's `endpointPrefix`.
pub fn endpoint_prefix(model: &Model) -> Option<String> {
    model
        .schema()
        .service()
        .endpoint_prefix()
        .map(str::to_owned)
}

/// The name of the error shape whose code is `code`
/// (`ClientExceptionsFactory`'s class name).
pub fn error_shape_name(model: &Model, code: &str) -> Option<String> {
    let registry = model.schema().registry();
    registry
        .entry_for_error_code(code)
        .map(|schema| schema.id().name().to_owned())
}

/// One member of an operation's input structure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberInfo {
    pub name: String,
    /// The `location` trait (`header`, `querystring`, ...).
    pub location: Option<String>,
    /// The `locationName` trait.
    pub location_name: Option<String>,
    /// botocore's type name: `structure`, `list`, `string`, ...
    pub type_name: String,
    /// A list's member type name.
    pub item_type_name: Option<String>,
}

/// The members of `operation`'s input, in model order: none for an
/// operation without an input or that the model lacks.
pub fn input_members(model: &Model, operation: &str) -> Vec<MemberInfo> {
    let service = model.schema().service();
    let Some(input) = service.operation(operation).and_then(|found| found.input()) else {
        return Vec::new();
    };
    input
        .members()
        .map(|member| MemberInfo {
            name: member.member_name().unwrap_or_default().to_owned(),
            location: member.location().map(str::to_owned),
            location_name: member.location_name().map(str::to_owned),
            type_name: member.type_name().to_owned(),
            item_type_name: member.list_member().map(|item| item.type_name().to_owned()),
        })
        .collect()
}

/// `ParamValidator().validate`'s verdict on an input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Validated {
    Valid,
    /// `message`: `str(ParamValidationError)`; `report`: the report alone
    /// (`generate_report`).
    Invalid {
        message: String,
        report: String,
    },
    /// The reason the Python route must decide, as `{:?}`.
    Undecided(String),
}

/// `input` validated against `operation`.
///
/// # Errors
/// An operation the model lacks.
pub fn validate_case(model: &Model, operation: &str, input: &Value) -> Result<Validated, String> {
    let service = model.schema().service();
    let operation = service.operation(operation).ok_or("no operation")?;
    Ok(match validate_input(operation, input) {
        Validation::Valid => Validated::Valid,
        Validation::Invalid(report) => Validated::Invalid {
            message: report.message(),
            report: report.to_string(),
        },
        Validation::Undecided(reason) => Validated::Undecided(format!("{reason:?}")),
    })
}

// ---------------------------------------------------------------- protocols

/// A wire protocol.
#[derive(Clone, Copy, Debug)]
pub struct ProtocolHandle(&'static dyn ClientProtocol);

/// The protocol named `query`, `json` or `rest-json`.
pub fn protocol_named(name: &str) -> Option<ProtocolHandle> {
    IMPLEMENTED.named(name).map(ProtocolHandle)
}

/// The protocol a client of `model` speaks, when one is implemented.
pub fn protocol_for_model(model: &Model) -> Option<ProtocolHandle> {
    crate::protocol::for_model(model).map(ProtocolHandle)
}

fn no_operation(operation: &str) -> String {
    format!("no operation {operation}")
}

/// The request `protocol` serializes for `operation` with `input`.
///
/// # Errors
/// The serializer's error, or an operation the model lacks.
pub fn serialize_case(
    protocol: ProtocolHandle,
    model: &Model,
    operation: &str,
    input: &Value,
) -> Result<Serialized, SerializeError> {
    let found = (model.schema().service())
        .operation(operation)
        .ok_or_else(|| SerializeError::new(no_operation(operation)))?;
    protocol.0.serialize_request(found, input, model)
}

/// Parser options from a timestamp and a blob handler. Harnesses build
/// options only here, so the struct may grow.
pub fn parse_options<'a>(
    timestamp: TimestampHandler<'a>,
    blob: BlobHandler<'a>,
) -> ParseOptions<'a> {
    ParseOptions { timestamp, blob }
}

/// botocore's parser defaults (`DEFAULT_TIMESTAMP_PARSER`, base64 blobs).
pub fn botocore_options() -> ParseOptions<'static> {
    ParseOptions::BOTOCORE
}

/// botocore's default timestamp parser over one wire value.
///
/// # Errors
/// A value the parser refuses.
pub fn botocore_timestamp(value: &Value) -> Result<Value, ParseError> {
    (ParseOptions::BOTOCORE.timestamp)(value)
}

/// botocore's default blob parser (base64) over one wire value.
///
/// # Errors
/// A value the parser refuses.
pub fn botocore_blob(value: &Value) -> Result<Value, ParseError> {
    (ParseOptions::BOTOCORE.blob)(value)
}

/// The output `protocol` parses from a successful `response`.
///
/// # Errors
/// The parser's error, or an operation the model lacks.
pub fn parse_case(
    protocol: ProtocolHandle,
    model: &Model,
    operation: &str,
    response: &HttpResponse,
    options: &ParseOptions<'_>,
) -> Result<Value, ParseError> {
    let found = (model.schema().service())
        .operation(operation)
        .ok_or_else(|| ParseError::new(no_operation(operation)))?;
    protocol.0.parse(found, response, options)
}

/// The service error `protocol` parses from an error `response`.
///
/// # Errors
/// The parser's error, or an operation the model lacks.
pub fn parse_error_case(
    protocol: ProtocolHandle,
    model: &Model,
    operation: &str,
    response: &HttpResponse,
    options: &ParseOptions<'_>,
) -> Result<ServiceError, ParseError> {
    let found = (model.schema().service())
        .operation(operation)
        .ok_or_else(|| ParseError::new(no_operation(operation)))?;
    let registry = model.schema().registry();
    protocol.0.parse_error(found, response, options, registry)
}

// ---------------------------------------------------------------- endpoints

/// The service's endpoint rules and partitions in `source`.
///
/// # Errors
/// The loader's error, as text.
pub fn endpoint_data(
    source: &dyn Source,
    service: &str,
    model: &Model,
) -> Result<EndpointData, String> {
    EndpointData::load(&Dyn(source), service, model.schema().service())
        .map_err(|error| error.to_string())
}

/// Endpoint data from the documents' bytes: the rule set, `partitions.json`
/// and, with the service's endpoint prefix, `endpoints.json`.
///
/// # Errors
/// The reader's error, as text.
pub fn endpoint_data_from_documents(
    rules: Vec<u8>,
    partitions: Vec<u8>,
    endpoints: Option<(&[u8], &str)>,
) -> Result<EndpointData, String> {
    EndpointData::from_documents(rules, partitions, endpoints).map_err(|error| error.to_string())
}

/// A rule parameter's value.
pub use crate::endpoint::ParamValue;

/// A service's endpoint data for the endpoint suite: what a client resolves
/// with.
#[derive(Clone, Debug)]
pub struct RuleSet {
    data: EndpointData,
}

/// The service's rule set, partitions and `endpoints.json` in `source`, as
/// [`endpoint_data`] loads them.
///
/// # Errors
/// The loader's error, as text.
pub fn rule_set(source: &dyn Source, service: &str, model: &Model) -> Result<RuleSet, String> {
    use model::source::{load_data_bytes, load_service_bytes};
    let source = Dyn(source);
    let text = |error: LoadError| error.to_string();
    let (_, rules) = load_service_bytes(&source, service, Kind::EndpointRuleSet).map_err(text)?;
    let partitions = load_data_bytes(&source, "partitions").map_err(text)?;
    let endpoints = load_data_bytes(&source, "endpoints").map_err(text)?;
    let prefix = model
        .schema()
        .service()
        .endpoint_prefix()
        .unwrap_or(service);
    let data = EndpointData::from_documents(rules, partitions, Some((&endpoints, prefix)))
        .map_err(|error| error.to_string())?;
    Ok(RuleSet { data })
}

/// What one resolution gives: an endpoint, or the error text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EndpointOutcome {
    Endpoint {
        url: String,
        /// The endpoint's properties, as JSON text.
        properties: String,
        /// How many headers the endpoint carries.
        headers: usize,
    },
    /// A rule's error message, or the engine's failure.
    Error(String),
}

/// `EndpointProvider.resolve_endpoint(**params)` over `rules`.
pub fn resolve_case(rules: &RuleSet, params: &[(&str, ParamValue)]) -> EndpointOutcome {
    let params = params.iter().map(|(name, value)| (*name, value));
    match crate::endpoint::evaluate(&rules.data, params) {
        Ok(endpoint_rules::Outcome::Endpoint(endpoint)) => EndpointOutcome::Endpoint {
            url: endpoint.url,
            properties: endpoint.properties.to_string(),
            headers: endpoint.headers.len(),
        },
        Ok(endpoint_rules::Outcome::Error(message)) => EndpointOutcome::Error(message),
        Err(text) => EndpointOutcome::Error(text),
    }
}

/// A call's endpoint: the parameters a client of `model` gives `operation`
/// with `input` and these built-ins alone (`AWS::Auth::AccountId` among
/// them as the credentials' account id), resolved as [`resolve_case`] does.
///
/// # Errors
/// An operation the model lacks.
pub fn endpoint_case(
    rules: &RuleSet,
    model: &Model,
    operation: &str,
    input: &Value,
    builtins: &[(&str, ParamValue)],
) -> Result<EndpointOutcome, String> {
    let found = (model.schema().service())
        .operation(operation)
        .ok_or_else(|| no_operation(operation))?;
    let mut set = crate::endpoint::Builtins::default();
    let mut account_id = None;
    for (name, value) in builtins {
        match (*name, value) {
            ("AWS::Auth::AccountId", ParamValue::String(id)) => account_id = Some(id.as_str()),
            _ => set.set(name, value.clone()),
        }
    }
    let params = crate::endpoint::provider_params(&rules.data, found, input, &set, account_id);
    let params: Vec<(&str, ParamValue)> = params
        .iter()
        .map(|(name, value)| (name, value.clone()))
        .collect();
    Ok(resolve_case(rules, &params))
}

// ---------------------------------------------------------------- census

/// A refusal's kind and what it names.
fn row(refusal: &Unsupported) -> (String, String) {
    let named = |kind: &str, name: &str| (kind.to_owned(), name.to_owned());
    match refusal {
        Unsupported::Protocol(name) => named("protocol", name),
        Unsupported::Trait(name) => named("trait", name),
        Unsupported::Builtin(name) => named("builtin", name),
        Unsupported::ClientContextParam(name) => named("clientContextParam", name),
        Unsupported::Auth(name) => named("auth", name),
        Unsupported::Hook(reason) => named("hook", reason),
        Unsupported::OperationContextParams => named("operationContextParams", "-"),
        Unsupported::UnsignedPayload => named("unsignedPayload", "-"),
        Unsupported::EndpointHeaders => named("endpointHeaders", "-"),
        Unsupported::RegionPattern => named("regionPattern", "-"),
        Unsupported::EndpointData => named("endpointData", "-"),
        Unsupported::Validation(undecided) => named("validation", &format!("{undecided:?}")),
        Unsupported::StartingToken(why) => named("startingToken", why),
        Unsupported::Paginator(Refused::Expression(expression)) => named("paginator", expression),
        Unsupported::Paginator(Refused::Malformed(why)) => named("paginator", why),
    }
}

/// An operation's name and its static refusal's (kind, name), if any.
pub type Refusal = (String, Option<(String, String)>);

/// Every operation of `service` in `source`, in model order, with the
/// static refusal (kind, name) a client with `customizations` gives it.
///
/// # Errors
/// A model, endpoint or paginator load error, as text.
pub fn refusals(
    source: &dyn Source,
    service: &str,
    customizations: &[&'static Customization],
) -> Result<Vec<Refusal>, String> {
    let source = Dyn(source);
    let text = |error: &dyn std::fmt::Display| error.to_string();
    let model = Model::load(&source, service).map_err(|e| text(&e))?;
    let schema = model.schema().service();
    let endpoints = EndpointData::load(&source, service, schema).map_err(|e| text(&e))?;
    let pages = Paginators::load(&source, service).map_err(|e| text(&e))?;
    Ok(schema
        .operations()
        .map(|operation| {
            let entry = pages.get(operation.name()).map(|entry| &entry.config);
            let refusal = static_refusal(
                &model,
                operation,
                &endpoints,
                service,
                customizations,
                entry,
            );
            (operation.name().to_owned(), refusal.as_ref().map(row))
        })
        .collect())
}

// ---------------------------------------------------------------- clients

/// Signing keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub account_id: Option<String>,
}

/// Where a case's client gets its credentials.
#[derive(Clone, Debug)]
pub enum Credentials {
    /// These keys on every load, reported as found by `method`.
    Static {
        keys: Keys,
        method: Option<&'static str>,
    },
    /// None found (`NoCredentialsError`).
    Absent,
    /// Every load fails with this text.
    Failing(String),
    /// These keys, counting each load.
    Counting { keys: Keys, loads: Arc<AtomicUsize> },
}

impl Keys {
    fn signing(&self) -> SigningCredentials {
        SigningCredentials {
            access_key_id: self.access_key_id.clone(),
            secret_access_key: self.secret_access_key.clone(),
            session_token: self.session_token.clone(),
            account_id: self.account_id.clone(),
        }
    }
}

impl CredentialSource for Credentials {
    fn credentials(&self) -> Result<Option<SigningCredentials>, CredentialError> {
        match self {
            Self::Static { keys, .. } => Ok(Some(keys.signing())),
            Self::Absent => Ok(None),
            Self::Failing(text) => Err(CredentialError(text.clone())),
            Self::Counting { keys, loads } => {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok(Some(keys.signing()))
            }
        }
    }
    fn method(&self) -> Option<&'static str> {
        match self {
            Self::Static { method, .. } => *method,
            _ => None,
        }
    }
}

/// The endpoint a case's client resolves with; the client signs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EndpointCase {
    pub region: Option<String>,
    pub endpoint_url: Option<String>,
    pub use_fips: Option<bool>,
    pub use_dual_stack: Option<bool>,
}

/// What a case's client puts in its `User-Agent`.
#[derive(Clone, Debug)]
pub struct UserAgentCase {
    pub host: Host,
    pub distribution: Option<Distribution>,
    pub versions: Versions<'static>,
    /// `md/installer#`, `md/prompt#` and `md/command#`.
    pub installer: &'static str,
    pub prompt: &'static str,
    pub command: String,
}

/// A client as the CLI builds one, over an in-memory transport and a fixed
/// clock, with botocore's parser defaults.
#[derive(Debug)]
pub struct ClientCase {
    pub service: String,
    pub model: Arc<Model>,
    pub endpoints: EndpointData,
    pub endpoint: EndpointCase,
    pub settings: ClientSettings,
    pub credentials: Credentials,
    pub transport: Arc<InMemoryTransport>,
    pub now: Timestamp,
    pub random: f64,
    pub customizations: &'static [&'static Customization],
    pub user_agent: UserAgentCase,
}

/// The case's client.
///
/// # Errors
/// What `Client::new` refuses.
pub fn client_case(case: ClientCase) -> Result<Client, CallError> {
    let endpoint = EndpointConfig::new(&EndpointInputs {
        region: case.endpoint.region.as_deref(),
        use_fips: case.endpoint.use_fips,
        use_dual_stack: case.endpoint.use_dual_stack,
        endpoint_url: case.endpoint.endpoint_url.as_deref(),
        signing: true,
    });
    let user_agent = case.user_agent;
    Client::new(crate::call::ClientParts {
        service: case.service,
        model: case.model,
        endpoints: case.endpoints,
        endpoint,
        settings: case.settings,
        credentials: Arc::new(case.credentials),
        transport: case.transport,
        runtime: Arc::new(FixedRuntime::new(case.now, case.random)),
        customizations: case.customizations,
        parse: crate::protocol::ParseHooks::botocore(),
        user_agent: UserAgentContext {
            host: user_agent.host,
            distribution: user_agent.distribution,
            versions: user_agent.versions,
            extras: SessionExtras {
                installer: user_agent.installer,
                prompt: user_agent.prompt,
                command: user_agent.command,
            },
        },
    })
}

/// The error a call fails with when a provider's credential retrieval
/// fails (botocore's `CredentialRetrievalError`), as the client's chain
/// hands it to the call.
pub fn credential_retrieval_error(provider: &'static str, message: &str) -> CallError {
    let error = credentials::LoadError::CredentialRetrieval {
        provider,
        error_msg: message.to_owned(),
    };
    CallError::Credentials(error.to_string())
}

/// One call: its parsed output and `RetryAttempts`.
///
/// # Errors
/// The call's error.
pub fn call_case(
    client: &Client,
    operation: &str,
    input: Value,
) -> Result<(Value, u32), CallError> {
    client
        .call(operation, input)
        .map(|output| (output.value, output.retry_attempts))
}

/// `paginate`'s keyword arguments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaginationCase {
    pub max_items: Option<Value>,
    pub page_size: Option<Value>,
    pub starting_token: Option<String>,
}

/// Every page of `operation` with its paginator entry in `source`, then
/// `build_full_result`.
///
/// # Errors
/// Outer: a paginator load error, an operation without a paginator, or a
/// refused paginator (`{:?}`), as text. Inner: the pages' error.
pub fn paginate_case(
    client: &Client,
    source: &dyn Source,
    service: &str,
    operation: &str,
    input: Value,
    pagination: PaginationCase,
) -> Result<Result<Value, CallError>, String> {
    use crate::paginate::{PageIterator, PaginationConfig};
    let pages = Paginators::load(&Dyn(source), service).map_err(|error| error.to_string())?;
    let entry = pages
        .get(operation)
        .ok_or("the operation has no paginator")?;
    let config = entry
        .config
        .clone()
        .map_err(|refused| format!("{refused:?}"))?;
    let pagination = PaginationConfig {
        max_items: pagination.max_items,
        page_size: pagination.page_size,
        starting_token: pagination.starting_token,
    };
    Ok(
        PageIterator::new(client, operation, input, config, pagination)
            .and_then(|mut pages| pages.build_full_result()),
    )
}

// ---------------------------------------------------------------- readers

/// Whether `ElementTree.XMLParser` reads `bytes` to a tree.
pub fn xml_parses(bytes: &[u8]) -> bool {
    crate::xml::parse(bytes).is_ok()
}

/// The starting token's key, `boto_truncate_amount`.
pub const TRUNCATE_AMOUNT: &str = crate::paginate::TRUNCATE_AMOUNT;

/// A starting token as `_parse_starting_token` reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct StartingToken {
    /// The input tokens to send.
    pub token: Map,
    /// `boto_truncate_amount`, as the token gave it (0 when absent).
    pub index: Value,
}

/// `PageIterator._parse_starting_token` of `raw` for a paginator with
/// `input_token`.
///
/// # Errors
/// What `_parse_starting_token` raises.
pub fn parse_starting_token(raw: &str, input_token: &[String]) -> Result<StartingToken, CallError> {
    crate::paginate::parse_starting_token(raw, input_token).map(|parsed| StartingToken {
        token: parsed.token,
        index: parsed.index,
    })
}

/// The `resume_token` setter: `value` encoded as a starting token.
///
/// # Errors
/// What the setter raises, as text.
pub fn resume_token(value: &Map, input_token: &[String]) -> Result<String, String> {
    crate::paginate::resume_token(value, input_token).map_err(|error| format!("{error:?}"))
}

#[cfg(test)]
mod tests;
