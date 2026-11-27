//! The call loop against the in-memory transport, the fixed runtime and a
//! fake protocol.
use super::*;
use crate::credentials::{CredentialError, CredentialSource, SigningCredentials};
use crate::endpoint::{Builtins, EndpointConfig, EndpointData, EndpointInputs};
use crate::error::{CallError, ParseError, SerializeError, ServiceError};
use crate::hooks::{Customization, HookError, HookResult, Scope, SettingDefault};
use crate::http::{Headers, HttpRequest, HttpResponse, Method};
use crate::interceptors::{Intercept, InterceptorContext};
use crate::protocol::{BotocoreCompat, ClientProtocol, ParseOptions, Serialized};
use crate::retry::{Attempt, RetryDecision};
use crate::runtime_components::ClassifyRetry;
use crate::testing::{FixedRuntime, InMemoryTransport};
use crate::transport::TransportError;
use cli_core::client_config::ClientSettings;
use cli_core::client_config::user_agent::{Host, Versions};
use cli_core::client_config::{Resolved, Source};
use model::schema::OperationSchema;
use model::shapes::Model;
use model::value::{Map, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const SERVICE: &str = r#"{
    "metadata": {"protocol": "json", "endpointPrefix": "zfake", "signingName": "zfake",
        "signatureVersion": "v4", "serviceId": "ZFake", "jsonVersion": "1.0"},
    "operations": {
        "Op": {"name": "Op", "http": {"method": "POST", "requestUri": "/"},
            "input": {"shape": "OpInput"}},
        "Open": {"name": "Open", "http": {"method": "POST", "requestUri": "/"},
            "auth": ["smithy.api#noAuth"]},
        "Prefixed": {"name": "Prefixed", "http": {"method": "POST", "requestUri": "/"},
            "endpoint": {"hostPrefix": "data."}}
    },
    "shapes": {
        "OpInput": {"type": "structure", "members": {
            "Name": {"shape": "Short"},
            "Token": {"shape": "S", "idempotencyToken": true}}},
        "Short": {"type": "string", "min": 1},
        "S": {"type": "string"}
    }
}"#;

const RULES: &str = r#"{
    "version": "1.0",
    "parameters": {
        "Region": {"type": "String", "builtIn": "AWS::Region", "required": false},
        "Endpoint": {"type": "String", "builtIn": "SDK::Endpoint", "required": false}
    },
    "rules": [
        {"conditions": [{"fn": "isSet", "argv": [{"ref": "Endpoint"}]}],
         "endpoint": {"url": {"ref": "Endpoint"}}, "type": "endpoint"},
        {"conditions": [{"fn": "isSet", "argv": [{"ref": "Region"}]}],
         "endpoint": {"url": "https://zfake.{Region}.example.com"}, "type": "endpoint"},
        {"conditions": [], "error": "Missing Region", "type": "error"}
    ]
}"#;

const PARTITIONS: &str = r#"{"version": "1.1", "partitions": [{"id": "aws",
    "regionRegex": "^(us|eu)\\-\\w+\\-\\d+$", "regions": {},
    "outputs": {"name": "aws", "dnsSuffix": "example.com", "dualStackDnsSuffix": "example.com",
        "supportsFIPS": true, "supportsDualStack": true, "implicitGlobalRegion": "us-east-1"}}]}"#;

/// The fake protocol: a JSON-ish body of the input as given, two headers
/// of its own, the operation's host prefix as written, a body `bad` that
/// fails to parse, and the error code from `x-fake-code`.
#[derive(Debug)]
struct Fake;

impl ClientProtocol for Fake {
    fn protocol_id(&self) -> model::schema::TraitId {
        model::schema::traits::AWS_QUERY
    }
    fn payload_codec(&self) -> Option<&dyn model::serde::Codec> {
        None
    }
    fn compat(&self) -> &BotocoreCompat {
        &BotocoreCompat::RPC
    }
    fn serialize_request(
        &self,
        op: OperationSchema<'_>,
        input: &Value,
        _: &Model,
    ) -> Result<Serialized, SerializeError> {
        let body = model::pyjson::dumps(input, &model::pyjson::Options::compact(), None)
            .map_err(|error| SerializeError::new(error.to_string()))?;
        Ok(Serialized {
            method: Method::Post,
            path: "/".into(),
            query: String::new(),
            headers: Headers::from_pairs([
                ("X-Amz-Target", format!("ZFake.{}", op.name()).as_str()),
                ("Content-Type", "application/x-amz-json-1.0"),
            ]),
            body: body.into_bytes(),
            host_prefix: op.host_prefix().map(Into::into),
        })
    }
    fn deserialize_response(
        &self,
        _: OperationSchema<'_>,
        _: Option<model::schema::Schema<'_>>,
        _: &HttpResponse,
        _: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        Err(ParseError::new("the fake answers for the whole parse"))
    }
    fn parse_error_metadata(&self, _: &HttpResponse) -> Result<Map, ParseError> {
        Err(ParseError::new("the fake answers for the whole parse"))
    }
    fn deserialize_modeled_error(
        &self,
        _: model::schema::Schema<'_>,
        _: &HttpResponse,
        _: &ParseOptions<'_>,
    ) -> Result<Map, ParseError> {
        Err(ParseError::new("the fake answers for the whole parse"))
    }
    fn parse(
        &self,
        _: OperationSchema<'_>,
        response: &HttpResponse,
        _: &ParseOptions<'_>,
    ) -> Result<Value, ParseError> {
        if response.body == b"bad" {
            return Err(ParseError::new("Unable to parse response"));
        }
        if response.body == b"null" {
            return Ok(Value::Null);
        }
        let mut output = Map::new();
        output.insert(
            "Body".into(),
            Value::String(String::from_utf8_lossy(&response.body).into()),
        );
        Ok(Value::Map(output))
    }
    fn parse_error(
        &self,
        op: OperationSchema<'_>,
        response: &HttpResponse,
        _: &ParseOptions<'_>,
        _: model::schema::TypeRegistry<'_>,
    ) -> Result<ServiceError, ParseError> {
        if response.body == b"bad" {
            return Err(ParseError::new("Unable to parse response"));
        }
        let mut error = Map::new();
        if let Some(code) = response.headers.get("x-fake-code") {
            error.insert("Code".into(), Value::String(code.into()));
        }
        error.insert("Message".into(), Value::String("fake".into()));
        Ok(ServiceError {
            error,
            ..ServiceError::new(op.name(), response.status)
        })
    }
}

static FAKE: Fake = Fake;

#[derive(Debug)]
struct Keys(Option<SigningCredentials>);

impl CredentialSource for Keys {
    fn credentials(&self) -> Result<Option<SigningCredentials>, CredentialError> {
        Ok(self.0.clone())
    }
}

fn keys() -> Arc<Keys> {
    Arc::new(Keys(Some(SigningCredentials {
        access_key_id: "AKIDEXAMPLE".into(),
        secret_access_key: "SECRET".into(),
        session_token: Some("TOKEN".into()),
        account_id: None,
    })))
}

/// 2015-08-30T12:36:00Z.
fn clock() -> timestamp::Timestamp {
    timestamp::Timestamp::from_unix_seconds(1_440_938_160).unwrap_or_else(timestamp::Timestamp::now)
}

struct Fixture {
    transport: Arc<InMemoryTransport>,
    runtime: Arc<FixedRuntime>,
    client: Client,
}

fn fixture(
    settings: ClientSettings,
    credentials: Arc<dyn CredentialSource>,
    customizations: &'static [&'static Customization],
) -> TestResult<Fixture> {
    fixture_with(RULES, settings, credentials, customizations)
}

fn fixture_with(
    rules: &str,
    settings: ClientSettings,
    credentials: Arc<dyn CredentialSource>,
    customizations: &'static [&'static Customization],
) -> TestResult<Fixture> {
    fixture_of(SERVICE, rules, settings, credentials, customizations)
}

fn fixture_of(
    service: &str,
    rules: &str,
    settings: ClientSettings,
    credentials: Arc<dyn CredentialSource>,
    customizations: &'static [&'static Customization],
) -> TestResult<Fixture> {
    let model = Arc::new(Model::from_json(service.as_bytes())?);
    let endpoints = EndpointData::from_documents(
        rules.as_bytes().to_vec(),
        PARTITIONS.as_bytes().to_vec(),
        None,
    )?;
    let transport = Arc::new(InMemoryTransport::new());
    let runtime = Arc::new(FixedRuntime::new(clock(), 0.0));
    let signing = settings.signing.value;
    let client = Client::with_protocol(
        ClientParts {
            service: "zfake".into(),
            model,
            endpoints,
            endpoint: EndpointConfig::new(&EndpointInputs {
                region: Some("us-west-2"),
                signing,
                ..EndpointInputs::default()
            }),
            settings,
            credentials,
            transport: transport.clone(),
            runtime: runtime.clone(),
            customizations,
            parse: crate::protocol::ParseHooks::botocore(),
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
                extras: crate::useragent::SessionExtras {
                    installer: "other",
                    prompt: "off",
                    command: "zfake.op".into(),
                },
            },
        },
        &FAKE,
    )?;
    Ok(Fixture {
        transport,
        runtime,
        client,
    })
}

fn settings() -> ClientSettings {
    ClientSettings::new("zfake.op")
}

fn ok(body: &str) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: Headers::new(),
        body: body.as_bytes().to_vec(),
    }
}

fn status(status: u16, code: &str) -> HttpResponse {
    HttpResponse {
        status,
        headers: Headers::from_pairs([("x-fake-code", code)]),
        body: Vec::new(),
    }
}

fn input(name: &str) -> Value {
    let mut map = Map::new();
    map.insert("Name".into(), Value::String(name.into()));
    Value::Map(map)
}

fn names(request: &HttpRequest) -> Vec<&str> {
    request.headers.iter().map(|(name, _)| name).collect()
}

/// botocore's order on the wire: the serializer's headers, `User-Agent`,
/// the signer's, then the unsigned trio; the signature covers none of the
/// trio, which botocore adds after `RequestSigner.handler` has signed
/// (`add_retry_headers` on the generic `request-created`, then
/// `AWSRequestPreparer._prepare_headers`'s `Content-Length`).
#[test]
fn headers_go_out_in_botocore_s_order_with_the_trio_unsigned() -> TestResult {
    let f = fixture(settings(), keys(), &[])?;
    f.transport.respond(ok("{}"));
    f.client.call("Op", input("a"))?;
    let sent = f.transport.sent();
    let request = &sent[0].0;
    assert_eq!(
        names(request),
        [
            "X-Amz-Target",
            "Content-Type",
            "User-Agent",
            "X-Amz-Date",
            "X-Amz-Security-Token",
            "Authorization",
            "amz-sdk-invocation-id",
            "amz-sdk-request",
            "Content-Length",
        ]
    );
    assert_eq!(
        request.url.to_string(),
        "https://zfake.us-west-2.example.com/"
    );
    let get = |name| request.headers.get(name).unwrap_or_default();
    assert_eq!(get("X-Amz-Date"), "20150830T123600Z");
    assert_eq!(
        get("amz-sdk-invocation-id"),
        "00000000-0000-4000-8000-000000000002"
    );
    assert_eq!(get("amz-sdk-request"), "attempt=1; max=3");
    assert_eq!(get("Content-Length"), request.body.len().to_string());
    assert!(
        get("Authorization").contains(
            "/20150830/us-west-2/zfake/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date;x-amz-security-token;x-amz-target, "
        ),
        "{}",
        get("Authorization")
    );
    assert_eq!(sent[0].1.connect, Duration::from_secs(60));
    Ok(())
}

/// `generate_idempotent_uuid`: an idempotency token left out is a uuid4,
/// drawn before the invocation id; one given is kept.
#[test]
fn idempotency_tokens_are_filled_only_when_absent() -> TestResult {
    let f = fixture(settings(), keys(), &[])?;
    f.transport.respond(ok("{}"));
    f.transport.respond(ok("{}"));
    f.client.call("Op", input("a"))?;
    let mut given = Map::new();
    given.insert("Token".into(), Value::String("mine".into()));
    f.client.call("Op", Value::Map(given))?;
    let bodies: Vec<String> = f
        .transport
        .sent()
        .iter()
        .map(|(request, _)| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    assert_eq!(
        bodies,
        [
            r#"{"Name":"a","Token":"00000000-0000-4000-8000-000000000001"}"#,
            r#"{"Token":"mine"}"#
        ]
    );
    Ok(())
}

/// A 500 is retried, after the backoff, with the same invocation id and
/// the attempt counted; the output says how many retries it took.
#[test]
fn a_server_error_is_retried_with_the_same_invocation_id() -> TestResult {
    let f = fixture(settings(), keys(), &[])?;
    f.transport.respond(status(500, "InternalError"));
    f.transport.respond(ok("done"));
    let output = f.client.call("Op", input("a"))?;
    let sent = f.transport.sent();
    let header = |index: usize, name| {
        sent[index]
            .0
            .headers
            .get(name)
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(sent.len(), 2);
    assert_eq!(
        header(0, "amz-sdk-invocation-id"),
        header(1, "amz-sdk-invocation-id")
    );
    assert_eq!(header(1, "amz-sdk-request"), "attempt=2; max=3");
    assert_eq!(f.runtime.slept(), [Duration::ZERO]);
    assert_eq!(output.retry_attempts, 1);
    assert_eq!(output.status, 200);
    Ok(())
}

/// A request botocore's session gets no response for is retried to the
/// attempt limit, and the error names the endpoint.
#[test]
fn a_transport_failure_is_retried_to_the_limit() -> TestResult {
    let f = fixture(settings(), keys(), &[])?;
    for _ in 0..3 {
        f.transport.fail(TransportError::Connect);
    }
    let error = f.client.call("Op", input("a")).err().ok_or("no error")?;
    assert_eq!(f.transport.sent().len(), 3);
    assert_eq!(
        error.to_string(),
        r#"Could not connect to the endpoint URL: "https://zfake.us-west-2.example.com/""#
    );
    Ok(())
}

/// A 403 is not retried; one that exhausts its attempts says so.
#[test]
fn a_client_error_is_not_retried_and_exhaustion_is_reported() -> TestResult {
    let f = fixture(settings(), keys(), &[])?;
    f.transport.respond(status(403, "AccessDenied"));
    let denied = f.client.call("Op", input("a")).err().ok_or("no error")?;
    assert_eq!(f.transport.sent().len(), 1);
    assert_eq!(
        denied.to_string(),
        "An error occurred (AccessDenied) when calling the Op operation: fake"
    );
    for _ in 0..3 {
        f.transport.respond(status(503, "ServiceUnavailable"));
    }
    let exhausted = f.client.call("Op", input("a")).err().ok_or("no error")?;
    assert_eq!(
        exhausted.to_string(),
        "An error occurred (ServiceUnavailable) when calling the Op operation \
         (reached max retries: 2): fake"
    );
    Ok(())
}

/// A response that fails to parse is the call's error at once: botocore's
/// `Endpoint._do_get_response` calls `parser.parse` outside its `try`, so a
/// `ResponseParserError` leaves `_send_request` before `_needs_retry` runs.
#[test]
fn a_parse_failure_is_not_retried() -> TestResult {
    let f = fixture(settings(), keys(), &[])?;
    f.transport.respond(ok("bad"));
    let error = f.client.call("Op", input("a")).err().ok_or("no error")?;
    assert!(matches!(error, CallError::Parse(_)), "{error:?}");
    assert_eq!(f.transport.sent().len(), 1);
    Ok(())
}

/// A parse that gives no dict (a json body of `null`) fails where
/// `Endpoint._do_get_response` updates it, before `needs-retry`.
#[test]
fn a_parse_to_none_fails_as_botocore_s_update_does() -> TestResult {
    let f = fixture(settings(), keys(), &[])?;
    f.transport.respond(ok("null"));
    let error = f.client.call("Op", input("a")).err().ok_or("no error")?;
    assert_eq!(
        error.to_string(),
        "'NoneType' object has no attribute 'update'"
    );
    assert_eq!(f.transport.sent().len(), 1);
    Ok(())
}

/// An error response whose body cannot be read is botocore's
/// `ResponseParserError`, raised before `needs-retry`: not retried, even
/// for a status that would be.
#[test]
fn an_unreadable_error_body_is_not_retried() -> TestResult {
    let f = fixture(settings(), keys(), &[])?;
    f.transport.respond(HttpResponse {
        status: 503,
        headers: Headers::new(),
        body: b"bad".to_vec(),
    });
    let error = f.client.call("Op", input("a")).err().ok_or("no error")?;
    assert!(matches!(error, CallError::Parse(_)), "{error:?}");
    assert_eq!(f.transport.sent().len(), 1);
    Ok(())
}

/// `_urljoin`: the serializer's host prefix goes before the endpoint's
/// host, and the request is signed for the prefixed host.
#[test]
fn a_host_prefix_goes_before_the_endpoint_s_host() -> TestResult {
    let f = fixture(settings(), keys(), &[])?;
    f.transport.respond(ok("{}"));
    f.client.call("Prefixed", Value::Map(Map::new()))?;
    let sent = f.transport.sent();
    let url = sent[0].0.url.to_string();
    assert_eq!(url, "https://data.zfake.us-west-2.example.com/");
    Ok(())
}

/// With attempts to spare, the retry quota ends the retries: 35 of 14.
#[test]
fn an_exhausted_quota_stops_retries() -> TestResult {
    let mut settings = settings();
    settings.max_attempts = Some(Resolved::new(100, Source::Env("AWS_MAX_ATTEMPTS")));
    let f = fixture(settings, keys(), &[])?;
    for _ in 0..40 {
        f.transport.respond(status(500, "InternalError"));
    }
    let error = f.client.call("Op", input("a")).err().ok_or("no error")?;
    assert_eq!(f.transport.sent().len(), 36);
    assert!(
        !error.to_string().contains("reached max retries"),
        "{error}"
    );
    Ok(())
}

/// `amz-sdk-request`'s `max` is the user's `max_attempts`, else a
/// customization's default, else 3.
#[test]
fn max_attempts_come_from_the_user_then_a_customization() -> TestResult {
    static FOUR: Customization = Customization {
        id: "four",
        scope: Scope::service("zfake"),
        setting_defaults: &[SettingDefault::MaxAttempts(4)],
        builtins: &[],
        interceptors: &[],
        retry_classifiers: &[],
        long_polling: false,
        replaces: &[],
    };
    static TABLE: [&Customization; 1] = [&FOUR];
    let header = |settings, table: &'static [&'static Customization]| -> TestResult<String> {
        let f = fixture(settings, keys(), table)?;
        f.transport.respond(ok("{}"));
        f.client.call("Op", input("a"))?;
        Ok(f.transport.sent()[0]
            .0
            .headers
            .get("amz-sdk-request")
            .unwrap_or_default()
            .to_owned())
    };
    assert_eq!(header(settings(), &TABLE)?, "attempt=1; max=4");
    let mut two = settings();
    two.max_attempts = Some(Resolved::new(2, Source::Profile("max_attempts")));
    assert_eq!(header(two, &TABLE)?, "attempt=1; max=2");
    assert_eq!(header(settings(), &[])?, "attempt=1; max=3");
    Ok(())
}

/// A retry after a response with a `Date` carries botocore's `ttl`: the
/// local time plus the read timeout plus the server's skew.
#[test]
fn a_retry_after_a_dated_response_carries_a_ttl() -> TestResult {
    let f = fixture(settings(), keys(), &[])?;
    let mut dated = status(500, "InternalError");
    dated.headers.set("Date", "Sun, 30 Aug 2015 12:36:10 GMT");
    f.transport.respond(dated);
    f.transport.respond(ok("{}"));
    f.client.call("Op", input("a"))?;
    let sent = f.transport.sent();
    assert_eq!(
        sent[1].0.headers.get("amz-sdk-request"),
        Some("ttl=20150830T123710Z; attempt=2; max=3")
    );
    Ok(())
}

/// A streaming input's retry carries a `ttl` as any other does:
/// `Endpoint._set_ttl` skips it for `has_streaming_input` in the retries
/// context, which botocore sets in the request context instead.
#[test]
fn a_streaming_input_s_retry_carries_a_ttl_too() -> TestResult {
    let service = SERVICE
        .replace(r#""protocol": "json""#, r#""protocol": "rest-json""#)
        .replace(
            r#""OpInput": {"type": "structure", "members": {"#,
            r#""OpInput": {"type": "structure", "payload": "Body", "members": {
            "Body": {"shape": "Stream"},"#,
        )
        .replace(
            r#""S": {"type": "string"}"#,
            r#""S": {"type": "string"}, "Stream": {"type": "blob", "streaming": true}"#,
        );
    let f = fixture_of(&service, RULES, settings(), keys(), &[])?;
    let mut dated = status(500, "InternalError");
    dated.headers.set("Date", "Sun, 30 Aug 2015 12:36:10 GMT");
    f.transport.respond(dated);
    f.transport.respond(ok("{}"));
    // The operation, not the input, decides: no body is given here.
    f.client.call("Op", input("a"))?;
    let sent = f.transport.sent();
    assert_eq!(
        sent[1].0.headers.get("amz-sdk-request"),
        Some("ttl=20150830T123710Z; attempt=2; max=3")
    );
    Ok(())
}

/// Unsigned: `--no-sign-request`, or an operation whose auth is
/// `smithy.api#noAuth`; signing without credentials is botocore's
/// `NoCredentialsError`, raised before anything is sent.
#[test]
fn unsigned_calls_and_missing_credentials() -> TestResult {
    let mut unsigned = settings();
    unsigned.signing = Resolved::new(false, Source::Flag("--no-sign-request"));
    let f = fixture(unsigned, keys(), &[])?;
    f.transport.respond(ok("{}"));
    f.client.call("Op", input("a"))?;
    let unsigned_names = [
        "X-Amz-Target",
        "Content-Type",
        "User-Agent",
        "amz-sdk-invocation-id",
        "amz-sdk-request",
        "Content-Length",
    ];
    assert_eq!(names(&f.transport.sent()[0].0), unsigned_names);
    let f = fixture(settings(), keys(), &[])?;
    f.transport.respond(ok("{}"));
    f.client.call("Open", Value::Map(Map::new()))?;
    assert!(!f.transport.sent()[0].0.headers.contains("Authorization"));
    let f = fixture(settings(), Arc::new(Keys(None)), &[])?;
    let error = f.client.call("Op", input("a")).err().ok_or("no error")?;
    assert!(matches!(error, CallError::NoCredentials), "{error:?}");
    assert!(f.transport.sent().is_empty());
    Ok(())
}

/// Validation fails the call before any request; [`Client::prepare`]
/// decides it, and everything else that needs no network, first.
#[test]
fn validation_is_decided_before_any_request() -> TestResult {
    let f = fixture(settings(), keys(), &[])?;
    let error = f.client.prepare("Op", input("")).err().ok_or("no error")?;
    assert!(
        error
            .to_string()
            .starts_with("Parameter validation failed:\nInvalid length"),
        "{error}"
    );
    let unknown = f
        .client
        .prepare("Nope", input("a"))
        .err()
        .ok_or("no error")?;
    assert!(
        matches!(unknown, CallError::UnknownOperation(_)),
        "{unknown:?}"
    );
    assert!(f.transport.sent().is_empty());
    Ok(())
}

/// Records each hook and classification it sees, in a slot they all share.
#[derive(Debug)]
struct Recorder(&'static str);

#[derive(Debug, Default)]
struct Trail(Vec<String>);

static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

impl Recorder {
    fn note(&self, cx: &mut InterceptorContext<'_>, stage: &str) {
        cx.state::<Trail>().0.push(format!("{}:{stage}", self.0));
        let trail = cx.state::<Trail>().0.join(" ");
        let mut log = LOG
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        log.clear();
        log.push(trail);
    }
}

impl Intercept for Recorder {
    fn name(&self) -> &'static str {
        self.0
    }
    fn modify_before_serialization(
        &self,
        cx: &mut InterceptorContext<'_>,
        _: &mut Value,
    ) -> HookResult {
        self.note(cx, "modify_before_serialization");
        Ok(())
    }
    fn modify_endpoint_builtins(
        &self,
        cx: &mut InterceptorContext<'_>,
        _: &mut crate::endpoint::Builtins,
    ) -> HookResult {
        self.note(cx, "modify_endpoint_builtins");
        Ok(())
    }
    fn modify_before_retry_loop(
        &self,
        cx: &mut InterceptorContext<'_>,
        _: &mut HttpRequest,
    ) -> HookResult {
        self.note(cx, "modify_before_retry_loop");
        Ok(())
    }
    fn modify_before_signing(
        &self,
        cx: &mut InterceptorContext<'_>,
        _: &mut HttpRequest,
    ) -> HookResult {
        self.note(cx, "modify_before_signing");
        Ok(())
    }
    fn modify_before_transmit(
        &self,
        cx: &mut InterceptorContext<'_>,
        _: &mut HttpRequest,
    ) -> HookResult {
        self.note(cx, "modify_before_transmit");
        Ok(())
    }
    fn short_circuit_transmit(
        &self,
        cx: &mut InterceptorContext<'_>,
        _: &HttpRequest,
    ) -> Result<Option<HttpResponse>, crate::hooks::HookError> {
        self.note(cx, "short_circuit_transmit");
        Ok((self.0 == "short").then(|| ok("canned")))
    }
    fn modify_before_deserialization(
        &self,
        cx: &mut InterceptorContext<'_>,
        _: &mut HttpResponse,
    ) -> HookResult {
        self.note(cx, "modify_before_deserialization");
        Ok(())
    }
    fn modify_before_completion(
        &self,
        cx: &mut InterceptorContext<'_>,
        _: &mut Result<Value, CallError>,
    ) -> HookResult {
        self.note(cx, "modify_before_completion");
        Ok(())
    }
}

impl ClassifyRetry for Recorder {
    fn name(&self) -> &'static str {
        self.0
    }
    fn classify_retry(
        &self,
        cx: &mut InterceptorContext<'_>,
        _: &Attempt<'_>,
    ) -> Option<RetryDecision> {
        self.note(cx, "classify_retry");
        None
    }
}

/// All eight hook positions and the retry classification run, in the
/// call's order, for customizations in scope only, sharing one typed slot;
/// `short_circuit_transmit` answering takes the place of sending.
#[test]
fn every_stage_runs_in_order_in_scope_and_can_short_circuit() -> TestResult {
    static ZFAKE: Recorder = Recorder("short");
    static OTHER: Recorder = Recorder("other");
    static IN_SCOPE: Customization = Customization {
        id: "in-scope",
        scope: Scope::operation("zfake", "Op"),
        setting_defaults: &[],
        builtins: &[],
        interceptors: &[&ZFAKE],
        retry_classifiers: &[&ZFAKE],
        long_polling: false,
        replaces: &[],
    };
    static OUT_OF_SCOPE: Customization = Customization {
        id: "out",
        scope: Scope::service("zother"),
        setting_defaults: &[],
        builtins: &[],
        interceptors: &[&OTHER],
        retry_classifiers: &[],
        long_polling: false,
        replaces: &[],
    };
    static TABLE: [&Customization; 2] = [&IN_SCOPE, &OUT_OF_SCOPE];
    let f = fixture(settings(), keys(), &TABLE)?;
    let output = f.client.call("Op", input("a"))?;
    assert!(f.transport.sent().is_empty());
    assert_eq!(output.value, {
        let mut map = Map::new();
        map.insert("Body".into(), Value::String("canned".into()));
        Value::Map(map)
    });
    let trail = LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("");
    assert_eq!(
        trail,
        "short:modify_before_serialization short:modify_endpoint_builtins \
         short:modify_before_retry_loop short:modify_before_signing short:modify_before_transmit \
         short:short_circuit_transmit short:modify_before_deserialization short:classify_retry \
         short:modify_before_completion"
    );
    Ok(())
}

/// Credentials that count how often they are asked for.
#[derive(Debug, Default)]
struct Counted {
    asked: std::sync::atomic::AtomicUsize,
    account_id: Option<String>,
}

impl Counted {
    fn asked(&self) -> usize {
        self.asked.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl CredentialSource for Counted {
    fn credentials(&self) -> Result<Option<SigningCredentials>, CredentialError> {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Some(SigningCredentials {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "SECRET".into(),
            session_token: None,
            account_id: self.account_id.clone(),
        }))
    }
}

/// `prepare` decides every refusal without credentials, so a caller that
/// falls back to Python after it has run no credential provider; `send`
/// asks for them to sign.
#[test]
fn prepare_asks_for_no_credentials() -> TestResult {
    let counted = Arc::new(Counted::default());
    let f = fixture(settings(), counted.clone(), &[])?;
    let prepared = f.client.prepare("Op", input("a"))?;
    assert_eq!(counted.asked(), 0);
    f.transport.respond(ok("{}"));
    f.client.send(prepared)?;
    assert!(counted.asked() >= 1);
    Ok(())
}

const ACCOUNT_RULES: &str = r#"{
    "version": "1.0",
    "parameters": {
        "Region": {"type": "String", "builtIn": "AWS::Region", "required": false},
        "AccountId": {"type": "String", "builtIn": "AWS::Auth::AccountId", "required": false}
    },
    "rules": [
        {"conditions": [{"fn": "isSet", "argv": [{"ref": "AccountId"}]}],
         "endpoint": {"url": "https://{AccountId}.zfake.{Region}.example.com"}, "type": "endpoint"},
        {"conditions": [{"fn": "isSet", "argv": [{"ref": "Region"}]}],
         "endpoint": {"url": "https://zfake.{Region}.example.com"}, "type": "endpoint"},
        {"conditions": [], "error": "Missing Region", "type": "error"}
    ]
}"#;

/// `AWS::Auth::AccountId` is deferred (`credentials.get_deferred_property`):
/// prepare resolves without it and asks no credentials; send asks, and an
/// account id the credentials carry gives the endpoint it selects.
#[test]
fn the_deferred_account_id_resolves_when_the_call_is_sent() -> TestResult {
    for (account_id, url) in [
        (
            Some("123456789012"),
            "https://123456789012.zfake.us-west-2.example.com/",
        ),
        (None, "https://zfake.us-west-2.example.com/"),
    ] {
        let counted = Arc::new(Counted {
            account_id: account_id.map(str::to_owned),
            ..Counted::default()
        });
        let f = fixture_with(ACCOUNT_RULES, settings(), counted.clone(), &[])?;
        let prepared = f.client.prepare("Op", input("a"))?;
        assert_eq!(counted.asked(), 0);
        f.transport.respond(ok("{}"));
        f.client.send(prepared)?;
        let sent = f.transport.sent();
        assert_eq!(sent[0].0.url.to_string(), url);
    }
    Ok(())
}

/// Refuses at `modify_endpoint_builtins` from its second call on: prepare's
/// resolution passes, the account-id one in `send` refuses.
#[derive(Debug)]
struct RefusesAgain(std::sync::atomic::AtomicUsize);

impl Intercept for RefusesAgain {
    fn name(&self) -> &'static str {
        "refuses-again"
    }
    fn modify_endpoint_builtins(
        &self,
        _: &mut InterceptorContext<'_>,
        _: &mut Builtins,
    ) -> HookResult {
        match self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
            0 => Ok(()),
            _ => Err(HookError::Refuse("not ported".into())),
        }
    }
}

static REFUSES_AGAIN: RefusesAgain = RefusesAgain(std::sync::atomic::AtomicUsize::new(0));
static REFUSE_AGAIN: Customization = Customization {
    id: "again",
    scope: Scope::ALL,
    setting_defaults: &[],
    builtins: &[],
    interceptors: &[&REFUSES_AGAIN],
    retry_classifiers: &[],
    long_polling: false,
    replaces: &[],
};
static AGAIN: [&Customization; 1] = [&REFUSE_AGAIN];

/// An account-id endpoint that carries headers the client does not send.
const ACCOUNT_HEADER_RULES: &str = r#"{
    "version": "1.0",
    "parameters": {
        "Region": {"type": "String", "builtIn": "AWS::Region", "required": false},
        "AccountId": {"type": "String", "builtIn": "AWS::Auth::AccountId", "required": false}
    },
    "rules": [
        {"conditions": [{"fn": "isSet", "argv": [{"ref": "AccountId"}]}],
         "endpoint": {"url": "https://{AccountId}.zfake.{Region}.example.com",
             "headers": {"x-zfake-account": ["{AccountId}"]}}, "type": "endpoint"},
        {"conditions": [{"fn": "isSet", "argv": [{"ref": "Region"}]}],
         "endpoint": {"url": "https://zfake.{Region}.example.com"}, "type": "endpoint"},
        {"conditions": [], "error": "Missing Region", "type": "error"}
    ]
}"#;

/// `send` resolves the endpoint again once credentials carry an account id,
/// after a credential provider has run: a refusal there can no longer leave
/// the call to Python. A customization's becomes its failure, and the rule
/// set's names the defect that let prepare admit the call; neither reads as
/// "not supported natively", and nothing is sent.
#[test]
fn a_refusal_when_send_resolves_again_fails_the_call() -> TestResult {
    let counted = || {
        Arc::new(Counted {
            account_id: Some("123456789012".into()),
            ..Counted::default()
        })
    };
    let f = fixture_with(ACCOUNT_RULES, settings(), counted(), &AGAIN)?;
    let prepared = f.client.prepare("Op", input("a"))?;
    let failed = f.client.send(prepared).err();
    let refused = |text: &str| text.contains("not ported");
    assert!(
        matches!(&failed, Some(CallError::Hook(HookError::Failed(text))) if refused(text)),
        "{failed:?}"
    );
    assert!(f.transport.sent().is_empty());
    let f = fixture_with(ACCOUNT_HEADER_RULES, settings(), counted(), &[])?;
    let prepared = f.client.prepare("Op", input("a"))?;
    let failed = f.client.send(prepared).err();
    assert!(
        matches!(&failed, Some(CallError::Internal(text)) if text.contains("endpoint headers")),
        "{failed:?}"
    );
    let text = failed.map(|error| error.to_string()).unwrap_or_default();
    assert!(!text.contains("not supported natively"), "{text}");
    assert!(f.transport.sent().is_empty());
    Ok(())
}

/// Refuses at the hook it names.
#[derive(Debug)]
struct Refuser(&'static str);

impl Intercept for Refuser {
    fn name(&self) -> &'static str {
        self.0
    }
    fn modify_before_serialization(
        &self,
        _: &mut InterceptorContext<'_>,
        _: &mut Value,
    ) -> HookResult {
        match self.0 {
            "modify_before_serialization" => Err(HookError::Refuse("not ported".into())),
            _ => Ok(()),
        }
    }
    fn modify_before_signing(
        &self,
        _: &mut InterceptorContext<'_>,
        _: &mut HttpRequest,
    ) -> HookResult {
        match self.0 {
            "modify_before_signing" => Err(HookError::Refuse("not ported".into())),
            _ => Ok(()),
        }
    }
}

static REFUSE_EARLY: Customization = Customization {
    id: "early",
    scope: Scope::ALL,
    setting_defaults: &[],
    builtins: &[],
    interceptors: &[&Refuser("modify_before_serialization")],
    retry_classifiers: &[],
    long_polling: false,
    replaces: &[],
};
static REFUSE_LATE: Customization = Customization {
    id: "late",
    scope: Scope::ALL,
    setting_defaults: &[],
    builtins: &[],
    interceptors: &[&Refuser("modify_before_signing")],
    retry_classifiers: &[],
    long_polling: false,
    replaces: &[],
};
static EARLY: [&Customization; 1] = [&REFUSE_EARLY];
static LATE: [&Customization; 1] = [&REFUSE_LATE];

/// A customization may leave a call to Python only before anything is
/// sent: in prepare's hooks the refusal is `Unsupported::Hook`; later it
/// is a failure of the call, since credentials have run.
#[test]
fn a_hook_refuses_only_before_the_call_starts() -> TestResult {
    let f = fixture(settings(), keys(), &EARLY)?;
    let refused = f.client.prepare("Op", input("a")).err();
    assert_eq!(
        refused,
        Some(CallError::Unsupported(Unsupported::Hook(
            "not ported".into()
        )))
    );
    let f = fixture(settings(), keys(), &LATE)?;
    f.transport.respond(ok("{}"));
    let failed = f.client.call("Op", input("a")).err();
    let late = |text: &str| text.contains("not ported");
    assert!(
        matches!(&failed, Some(CallError::Hook(HookError::Failed(text))) if late(text)),
        "{failed:?}"
    );
    assert!(f.transport.sent().is_empty());
    Ok(())
}

/// Credentials from `custom-process`, as a chain reports the provider that
/// answered.
#[derive(Debug)]
struct FromProcess;

impl CredentialSource for FromProcess {
    fn credentials(&self) -> Result<Option<SigningCredentials>, CredentialError> {
        Ok(Some(SigningCredentials {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "SECRET".into(),
            session_token: None,
            account_id: None,
        }))
    }
    fn method(&self) -> Option<&'static str> {
        Some("custom-process")
    }
}

/// The credential provider's feature codes join the `User-Agent` once it
/// has answered, as botocore's providers register them when the signer
/// asks for credentials and `rebuild_and_replace_user_agent_handler`
/// rebuilds the header after: `v` and `w` for a process, none unsigned.
#[test]
fn the_answering_provider_s_features_join_the_user_agent() -> TestResult {
    let features = |agent: &str| {
        agent
            .split(' ')
            .find_map(|part| part.strip_prefix("m/"))
            .unwrap_or("")
            .to_owned()
    };
    let f = fixture(settings(), Arc::new(FromProcess), &[])?;
    let prepared = f.client.prepare("Op", input("a"))?;
    f.transport.respond(ok("{}"));
    f.client.send(prepared)?;
    let sent = f.transport.sent();
    let agent = sent[0].0.headers.get("User-Agent").unwrap_or_default();
    let codes = features(agent);
    assert!(codes.split(',').any(|code| code == "v"), "{agent}");
    assert!(codes.split(',').any(|code| code == "w"), "{agent}");
    let mut unsigned = settings();
    unsigned.signing = Resolved::new(false, Source::Flag("--no-sign-request"));
    let f = fixture(unsigned, Arc::new(FromProcess), &[])?;
    f.transport.respond(ok("{}"));
    f.client.call("Op", input("a"))?;
    let sent = f.transport.sent();
    let agent = sent[0].0.headers.get("User-Agent").unwrap_or_default();
    let codes = features(agent);
    assert!(
        !codes.split(',').any(|code| code == "v" || code == "w"),
        "{agent}"
    );
    Ok(())
}

/// Marks its operation long-polling.
static LONG_POLL: Customization = Customization {
    id: "long-poll",
    scope: Scope::operation("zfake", "Op"),
    setting_defaults: &[],
    builtins: &[],
    interceptors: &[],
    retry_classifiers: &[],
    long_polling: true,
    replaces: &[],
};
static LONG_POLLS: [&Customization; 1] = [&LONG_POLL];

/// `RetryHandler.needs_retry`: once the quota refuses a retry, a
/// long-polling operation sleeps the delay it would have waited, then
/// stops; another operation stops at once.
#[test]
fn a_long_polling_operation_sleeps_when_the_quota_runs_out() -> TestResult {
    for (customizations, sleeps) in [(&LONG_POLLS[..], true), (&[][..], false)] {
        let mut settings = settings();
        settings.max_attempts = Some(Resolved::new(100, Source::Env("AWS_MAX_ATTEMPTS")));
        let f = fixture(settings, keys(), customizations)?;
        for _ in 0..40 {
            f.transport.respond(status(500, "InternalError"));
        }
        assert!(f.client.call("Op", input("a")).is_err());
        assert_eq!(f.transport.sent().len(), 36);
        // One sleep per retry, and with long polling one more at the end.
        assert_eq!(f.runtime.slept().len(), if sleeps { 36 } else { 35 });
    }
    Ok(())
}

/// An endpoint that names S3's signing name signs with S3's signer, and
/// the configured `payload_signing_enabled` reaches it: `UNSIGNED-PAYLOAD`
/// when false, the body's hash otherwise.
#[test]
fn an_s3_signed_call_honours_the_configured_payload_signing() -> TestResult {
    const S3_RULES: &str = r#"{"version": "1.0", "parameters": {},
        "rules": [{"conditions": [], "type": "endpoint", "endpoint": {
            "url": "https://zfake.us-west-2.example.com",
            "properties": {"authSchemes": [{"name": "sigv4", "signingName": "s3",
                "signingRegion": "us-west-2", "disableDoubleEncoding": true}]}}}]}"#;
    for (configured, expected) in [
        (Some(false), Some("UNSIGNED-PAYLOAD".to_owned())),
        (Some(true), None),
        (None, None),
    ] {
        let mut settings = settings();
        settings.payload_signing =
            configured.map(|value| Resolved::new(value, Source::S3("payload_signing_enabled")));
        let f = fixture_with(S3_RULES, settings, keys(), &[])?;
        f.transport.respond(ok("{}"));
        f.client.call("Op", input("a"))?;
        let sent = f.transport.sent();
        let request = &sent[0].0;
        let hash = request.headers.get("X-Amz-Content-SHA256");
        let body_hash = sigv4::sha256_hex(&request.body);
        assert_eq!(
            hash,
            Some(expected.as_deref().unwrap_or(&body_hash)),
            "{configured:?}"
        );
        let authorization = request.headers.get("Authorization").unwrap_or_default();
        assert!(
            authorization.contains("/us-west-2/s3/aws4_request"),
            "{authorization}"
        );
    }
    Ok(())
}
