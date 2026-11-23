//! The call loop against the in-memory transport, the fixed runtime and a
//! fake protocol.
use super::*;
use crate::credentials::{CredentialError, CredentialSource, SigningCredentials};
use crate::endpoint::{EndpointConfig, EndpointData, EndpointInputs};
use crate::error::{CallError, ParseError, SerializeError, ServiceError};
use crate::hooks::{CallContext, Customization, HookResult, Interceptor, Scope, SettingDefault};
use crate::http::{Headers, HttpRequest, HttpResponse, Method};
use crate::protocol::{Protocol, Serialized};
use crate::retry::{Attempt, RetryDecision};
use crate::testing::{FixedRuntime, InMemoryTransport};
use crate::transport::TransportError;
use cli_core::client_config::ClientSettings;
use cli_core::client_config::user_agent::{Host, Versions};
use cli_core::client_config::{Resolved, Source};
use model::shapes::{Model, Operation};
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
            "auth": ["smithy.api#noAuth"]}
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
/// of its own, a body `bad` that fails to parse, and the error code from
/// `x-fake-code`.
#[derive(Debug)]
struct Fake;

impl Protocol for Fake {
    fn name(&self) -> &'static str {
        "json"
    }
    fn serialize(
        &self,
        _: &Model,
        op: &Operation,
        input: &Value,
    ) -> Result<Serialized, SerializeError> {
        let body = model::pyjson::dumps(input, &model::pyjson::Options::compact(), None)
            .map_err(|error| SerializeError::new(error.to_string()))?;
        Ok(Serialized {
            method: Method::Post,
            path: "/".into(),
            query: String::new(),
            headers: Headers::from_pairs([
                ("X-Amz-Target", format!("ZFake.{}", op.name).as_str()),
                ("Content-Type", "application/x-amz-json-1.0"),
            ]),
            body: body.into_bytes(),
        })
    }
    fn parse(
        &self,
        _: &Model,
        _: &Operation,
        response: &HttpResponse,
    ) -> Result<Value, ParseError> {
        if response.body == b"bad" {
            return Err(ParseError::new("Unable to parse response"));
        }
        let mut output = Map::new();
        output.insert(
            "Body".into(),
            Value::String(String::from_utf8_lossy(&response.body).into()),
        );
        Ok(Value::Map(output))
    }
    fn parse_error(&self, _: &Model, op: &Operation, response: &HttpResponse) -> ServiceError {
        ServiceError {
            code: response.headers.get("x-fake-code").map(Into::into),
            message: Some("fake".into()),
            ..ServiceError::new(&op.name, response.status)
        }
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
    let model = Arc::new(Model::from_json(SERVICE.as_bytes())?);
    let endpoints = EndpointData::from_documents(
        RULES.as_bytes().to_vec(),
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

/// botocore's order on the wire (REPORT-m1 finding 1): the serializer's
/// headers, `User-Agent`, the signer's, then the unsigned trio; the
/// signature covers none of the trio.
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

/// A response that fails to parse is the call's error at once
/// (REPORT-m1 finding 4).
#[test]
fn a_parse_failure_is_not_retried() -> TestResult {
    let f = fixture(settings(), keys(), &[])?;
    f.transport.respond(ok("bad"));
    let error = f.client.call("Op", input("a")).err().ok_or("no error")?;
    assert!(matches!(error, CallError::Parse(_)), "{error:?}");
    assert_eq!(f.transport.sent().len(), 1);
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
        interceptors: &[],
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

/// Records each stage it sees, in a slot every stage shares.
#[derive(Debug)]
struct Recorder(&'static str);

#[derive(Debug, Default)]
struct Trail(Vec<String>);

static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

impl Recorder {
    fn note(&self, cx: &mut CallContext<'_>, stage: &str) {
        cx.slot::<Trail>().0.push(format!("{}:{stage}", self.0));
        let trail = cx.slot::<Trail>().0.join(" ");
        let mut log = LOG
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        log.clear();
        log.push(trail);
    }
}

impl Interceptor for Recorder {
    fn modify_input(&self, cx: &mut CallContext<'_>, _: &mut Value) -> HookResult {
        self.note(cx, "modify_input");
        Ok(())
    }
    fn endpoint_params(
        &self,
        cx: &mut CallContext<'_>,
        _: &mut crate::endpoint::EndpointParams,
    ) -> HookResult {
        self.note(cx, "endpoint_params");
        Ok(())
    }
    fn after_serialize(&self, cx: &mut CallContext<'_>, _: &mut HttpRequest) -> HookResult {
        self.note(cx, "after_serialize");
        Ok(())
    }
    fn select_auth(
        &self,
        cx: &mut CallContext<'_>,
        _: &crate::auth::AuthScheme,
    ) -> Option<crate::auth::AuthScheme> {
        self.note(cx, "select_auth");
        None
    }
    fn before_sign(&self, cx: &mut CallContext<'_>, _: &mut HttpRequest) -> HookResult {
        self.note(cx, "before_sign");
        Ok(())
    }
    fn before_transmit(
        &self,
        cx: &mut CallContext<'_>,
        _: &HttpRequest,
    ) -> Result<Option<HttpResponse>, crate::hooks::HookError> {
        self.note(cx, "before_transmit");
        Ok((self.0 == "short").then(|| ok("canned")))
    }
    fn after_receive(&self, cx: &mut CallContext<'_>, _: &mut HttpResponse) -> HookResult {
        self.note(cx, "after_receive");
        Ok(())
    }
    fn classify_retry(&self, cx: &mut CallContext<'_>, _: &Attempt<'_>) -> Option<RetryDecision> {
        self.note(cx, "classify_retry");
        None
    }
    fn modify_output(&self, cx: &mut CallContext<'_>, _: &mut Value) -> HookResult {
        self.note(cx, "modify_output");
        Ok(())
    }
}

/// All nine stages run, in the call's order, for interceptors in scope
/// only, sharing one typed slot; `before_transmit` answering takes the
/// place of sending.
#[test]
fn every_stage_runs_in_order_in_scope_and_can_short_circuit() -> TestResult {
    static ZFAKE: Recorder = Recorder("short");
    static OTHER: Recorder = Recorder("other");
    static IN_SCOPE: Customization = Customization {
        id: "in-scope",
        scope: Scope::operation("zfake", "Op"),
        setting_defaults: &[],
        interceptors: &[&ZFAKE],
        replaces: &[],
    };
    static OUT_OF_SCOPE: Customization = Customization {
        id: "out",
        scope: Scope::service("zother"),
        setting_defaults: &[],
        interceptors: &[&OTHER],
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
        "short:modify_input short:endpoint_params short:after_serialize short:select_auth \
         short:before_sign short:before_transmit short:after_receive short:classify_retry \
         short:modify_output"
    );
    Ok(())
}
