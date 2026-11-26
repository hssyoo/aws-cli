//! One test per behaviour; each names what it pins and what would break it.
use client::testing::facade::hooks::{self, Seen, Table};
use client::testing::facade::{
    self, CallError, Client, ClientCase, ClientSettings, Credentials, EndpointCase, FixedRuntime,
    Headers, Host, HttpResponse, InMemoryTransport, Keys, Model, PaginationCase, UserAgentCase,
    Validated, Versions,
};
use model::source::{Kind, LoadError, Source};
use model::value::Value;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use timestamp::Timestamp;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

// ---------------------------------------------------------------- fixtures

/// A JSON service: `Op` with an idempotency token, and `List`, paginated.
const SERVICE: &str = r#"{
    "metadata": {"protocol": "json", "endpointPrefix": "zfake", "signingName": "zfake",
        "signatureVersion": "v4", "serviceId": "ZFake", "jsonVersion": "1.0",
        "targetPrefix": "ZFake"},
    "operations": {
        "Op": {"name": "Op", "http": {"method": "POST", "requestUri": "/"},
            "input": {"shape": "OpInput"}},
        "List": {"name": "List", "http": {"method": "POST", "requestUri": "/"},
            "input": {"shape": "ListInput"}, "output": {"shape": "ListOutput"}}
    },
    "shapes": {
        "OpInput": {"type": "structure", "members": {
            "Name": {"shape": "S"},
            "Token": {"shape": "S", "idempotencyToken": true}}},
        "ListInput": {"type": "structure", "members": {"NextToken": {"shape": "S"}}},
        "ListOutput": {"type": "structure", "members": {
            "NextToken": {"shape": "S"}, "Items": {"shape": "Items"}}},
        "Items": {"type": "list", "member": {"shape": "S"}},
        "S": {"type": "string"}
    }
}"#;

const PAGINATORS: &str = r#"{"pagination": {"List": {"input_token": "NextToken",
    "output_token": "NextToken", "result_key": "Items"}}}"#;

const RULES: &str = r#"{
    "version": "1.0",
    "parameters": {
        "Region": {"type": "String", "builtIn": "AWS::Region", "required": false}
    },
    "rules": [
        {"conditions": [{"fn": "isSet", "argv": [{"ref": "Region"}]}],
         "endpoint": {"url": "https://zfake.{Region}.example.com"}, "type": "endpoint"},
        {"conditions": [], "error": "Missing Region", "type": "error"}
    ]
}"#;

const PARTITIONS: &str = r#"{"version": "1.1", "partitions": [{"id": "aws",
    "regionRegex": "^(us|eu)\\-\\w+\\-\\d+$", "regions": {},
    "outputs": {"name": "aws", "dnsSuffix": "example.com", "dualStackDnsSuffix": "example.com",
        "supportsFIPS": true, "supportsDualStack": true, "implicitGlobalRegion": "us-east-1"}}]}"#;

/// The paginators document of `zfake`, its only file.
struct Pages;

impl Source for Pages {
    fn services(&self, _: Kind) -> std::result::Result<Vec<String>, LoadError> {
        Ok(vec!["zfake".to_owned()])
    }
    fn versions(&self, _: &str, _: Kind) -> std::result::Result<Vec<String>, LoadError> {
        Ok(vec!["2020-01-01".to_owned()])
    }
    fn read(&self, name: &str) -> std::result::Result<Option<Vec<u8>>, LoadError> {
        let found = name == "zfake/2020-01-01/paginators-1";
        Ok(found.then(|| PAGINATORS.as_bytes().to_vec()))
    }
}

fn keys() -> Keys {
    Keys {
        access_key_id: "AKID".to_owned(),
        secret_access_key: "SECRET".to_owned(),
        session_token: None,
        account_id: None,
    }
}

fn signing() -> Credentials {
    Credentials::Static {
        keys: keys(),
        method: None,
    }
}

struct Fixture {
    client: Client,
    transport: Arc<InMemoryTransport>,
    runtime: Arc<FixedRuntime>,
}

fn fixture(credentials: Credentials, customizations: Table) -> Result<Fixture> {
    let model = facade::model_from_json(SERVICE.as_bytes())?;
    let endpoints = facade::endpoint_data_from_documents(
        RULES.as_bytes().to_vec(),
        PARTITIONS.as_bytes().to_vec(),
        None,
    )?;
    let transport = Arc::new(InMemoryTransport::new());
    let (client, runtime) = hooks::client_with_runtime(ClientCase {
        service: "zfake".to_owned(),
        model,
        endpoints,
        endpoint: EndpointCase {
            region: Some("us-west-2".to_owned()),
            ..EndpointCase::default()
        },
        settings: ClientSettings::new("zfake.op"),
        credentials,
        transport: transport.clone(),
        now: Timestamp::from_unix_seconds(1_440_938_160).ok_or("instant")?,
        random: 0.0,
        customizations,
        user_agent: UserAgentCase {
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
            installer: "other",
            prompt: "off",
            command: "zfake.op".to_owned(),
        },
    })?;
    Ok(Fixture {
        client,
        transport,
        runtime,
    })
}

fn respond(transport: &InMemoryTransport, status: u16, body: &str, times: usize) {
    for _ in 0..times {
        transport.respond(HttpResponse {
            status,
            headers: Headers::new(),
            body: body.as_bytes().to_vec(),
        });
    }
}

fn value(json: &serde_json::Value) -> Value {
    Value::from_serde(json)
}

fn keys_of(value: &Value) -> Vec<String> {
    match value {
        Value::Map(map) => map.keys().map(|key| key.text().into_owned()).collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------- the call loop

/// (1) The retry quota (`RetryQuotaChecker`, 500 at 14 a transient retry)
/// belongs to the client: calls and pages draw on the same one. Every
/// request fails with a 500, so each call or page first tries 3 times,
/// until the 35 retries the quota holds are spent: the 18th makes 2
/// requests, and every one after it 1. A quota made per call, per page or
/// per paginator would let each make 3. Another client has its own, full
/// quota: one kept for the process would leave it nothing.
#[test]
fn the_retry_quota_is_shared_by_a_client_s_calls_and_pages() -> Result {
    let f = fixture(signing(), &[])?;
    respond(&f.transport, 500, "{}", 100);
    let mut requests = Vec::new();
    for action in 0..20 {
        let before = f.transport.sent().len();
        if action % 2 == 0 {
            let called = facade::call_case(&f.client, "Op", value(&json!({"Name": "a"})));
            assert!(called.is_err(), "{called:?}");
        } else {
            let paged = facade::paginate_case(
                &f.client,
                &Pages,
                "zfake",
                "List",
                value(&json!({})),
                PaginationCase::default(),
            )?;
            assert!(paged.is_err(), "{paged:?}");
        }
        requests.push(f.transport.sent().len() - before);
    }
    let mut expected = vec![3; 17];
    expected.extend([2, 1, 1]);
    assert_eq!(requests, expected);
    assert_eq!(f.runtime.slept().len(), 35);
    let other = fixture(signing(), &[])?;
    respond(&other.transport, 500, "{}", 5);
    let called = facade::call_case(&other.client, "Op", value(&json!({"Name": "a"})));
    assert!(called.is_err(), "{called:?}");
    assert_eq!(other.transport.sent().len(), 3, "another client's quota");
    Ok(())
}

/// (2) An attempt that fails before it is sent, on its first try, ends the
/// call: a credential provider's error, no credentials found, and a
/// `before_sign` handler's exception are none of them retried, and the
/// call sleeps not at all.
#[test]
fn errors_before_the_first_send_are_not_retried() -> Result {
    type Check = fn(&CallError) -> bool;
    type Case = (&'static str, Credentials, (Table, Seen<u32>), Check);
    let cases: [Case; 3] = [
        (
            "credentials",
            Credentials::Failing("provider failed".to_owned()),
            hooks::before_sign_attempts(),
            |error| matches!(error, CallError::Credentials(text) if text == "provider failed"),
        ),
        (
            "no credentials",
            Credentials::Absent,
            hooks::before_sign_attempts(),
            |error| matches!(error, CallError::NoCredentials),
        ),
        (
            "before_sign",
            signing(),
            hooks::failing_before_sign("before-sign raised"),
            |error| {
                matches!(error, CallError::Hook(_)) && error.to_string() == "before-sign raised"
            },
        ),
    ];
    for (name, credentials, (table, seen), expected) in cases {
        let f = fixture(credentials, table)?;
        respond(&f.transport, 200, "{}", 5);
        let error = facade::call_case(&f.client, "Op", value(&json!({"Name": "a"})))
            .err()
            .ok_or(name)?;
        assert!(expected(&error), "{name}: {error:?}");
        assert_eq!(seen.entries(), [1], "{name}: attempts");
        assert_eq!(f.transport.sent().len(), 0, "{name}: requests");
        assert_eq!(f.runtime.slept(), [], "{name}: sleeps");
    }
    Ok(())
}

/// (3) Credentials load once per attempt, as botocore's signer asks the
/// provider each time it signs, and not at all before the first: a call
/// that takes 3 attempts loads them 3 times, and the next call, in 1,
/// once more. Loading once per call or once per client would load fewer.
#[test]
fn credentials_load_once_per_attempt() -> Result {
    let loads = Arc::new(AtomicUsize::new(0));
    let counting = Credentials::Counting {
        keys: keys(),
        loads: loads.clone(),
    };
    let f = fixture(counting, &[])?;
    respond(&f.transport, 500, "{}", 2);
    respond(&f.transport, 200, "{}", 2);
    let (_, retries) = facade::call_case(&f.client, "Op", value(&json!({"Name": "a"})))?;
    assert_eq!(retries, 2);
    assert_eq!(loads.load(Ordering::SeqCst), 3);
    let (_, retries) = facade::call_case(&f.client, "Op", value(&json!({"Name": "b"})))?;
    assert_eq!(retries, 0);
    assert_eq!(loads.load(Ordering::SeqCst), 4);
    assert_eq!(f.transport.sent().len(), 4);
    Ok(())
}

/// (4) Even a `modify_input` handler registered last runs before the
/// idempotency token is filled (`generate_idempotent_uuid` runs on
/// `before-parameter-build` after every handler a customization adds):
/// the handler sees the input without `Token`, and the request carries
/// the token, the call's first uuid.
#[test]
fn a_last_modify_input_hook_runs_before_the_token_fill() -> Result {
    let (table, seen) = hooks::last_modify_input();
    let f = fixture(signing(), table)?;
    respond(&f.transport, 200, "{}", 1);
    facade::call_case(&f.client, "Op", value(&json!({"Name": "a"})))?;
    assert_eq!(seen.entries(), [value(&json!({"Name": "a"}))]);
    let sent = f.transport.sent();
    let (request, _) = sent.first().ok_or("no request")?;
    let body: serde_json::Value = serde_json::from_slice(&request.body)?;
    assert_eq!(
        body,
        json!({"Name": "a", "Token": "00000000-0000-4000-8000-000000000001"})
    );
    Ok(())
}

/// (5) A `before_transmit` handler (`before-send`) that raises ends the
/// call with its error on the first attempt: nothing is sent, nothing
/// retried, no sleep. Treating it as a failed send would retry it.
#[test]
fn a_before_transmit_hook_error_is_not_retried() -> Result {
    let (table, seen) = hooks::failing_short_circuit("before-send raised");
    let f = fixture(signing(), table)?;
    respond(&f.transport, 200, "{}", 5);
    let error = facade::call_case(&f.client, "Op", value(&json!({"Name": "a"})))
        .err()
        .ok_or("no error")?;
    assert!(matches!(error, CallError::Hook(_)), "{error:?}");
    assert_eq!(error.to_string(), "before-send raised");
    assert_eq!(seen.entries(), [1]);
    assert_eq!(f.transport.sent().len(), 0);
    assert_eq!(f.runtime.slept(), []);
    Ok(())
}

// ---------------------------------------------------------------- protocols

/// A REST-JSON service: `Put` with a `headers` member that has no
/// `locationName`, `Get` whose output mixes header and body members, and
/// `Check` whose `required` list is out of member order.
const REST: &str = r#"{
    "metadata": {"protocol": "rest-json", "endpointPrefix": "zrest", "signingName": "zrest",
        "signatureVersion": "v4", "serviceId": "ZRest"},
    "operations": {
        "Put": {"name": "Put", "http": {"method": "PUT", "requestUri": "/put"},
            "input": {"shape": "PutInput"}, "output": {"shape": "PutOutput"}},
        "Get": {"name": "Get", "http": {"method": "GET", "requestUri": "/get"},
            "output": {"shape": "GetOutput"}},
        "Check": {"name": "Check", "http": {"method": "POST", "requestUri": "/check"},
            "input": {"shape": "CheckInput"}}
    },
    "shapes": {
        "PutInput": {"type": "structure", "members": {
            "Meta": {"shape": "Meta", "location": "headers"},
            "Data": {"shape": "S"}}},
        "PutOutput": {"type": "structure", "members": {
            "Meta": {"shape": "Meta", "location": "headers"}}},
        "GetOutput": {"type": "structure", "members": {
            "BodyOne": {"shape": "S"},
            "HeaderOne": {"shape": "S", "location": "header", "locationName": "x-one"},
            "BodyTwo": {"shape": "S"},
            "HeaderTwo": {"shape": "S", "location": "header", "locationName": "x-two"}}},
        "CheckInput": {"type": "structure", "required": ["Third", "First"], "members": {
            "First": {"shape": "S"}, "Second": {"shape": "S"}, "Third": {"shape": "S"}}},
        "Meta": {"type": "map", "key": {"shape": "S"}, "value": {"shape": "S"}},
        "S": {"type": "string"}
    }
}"#;

fn rest() -> Result<(Arc<Model>, facade::ProtocolHandle)> {
    let model = facade::model_from_json(REST.as_bytes())?;
    let protocol = facade::protocol_named("rest-json").ok_or("rest-json")?;
    Ok((model, protocol))
}

/// (6) A `headers` member without `locationName` serializes each entry
/// under the member's name as prefix (`_partition_parameters`' `key_name`
/// defaults to the member name), while parsing one reads every header,
/// its prefix empty (`_parse_header_map`'s `serialization.get('name', '')`).
#[test]
fn a_headers_member_without_location_name_is_prefixed_by_its_name() -> Result {
    let (model, protocol) = rest()?;
    let input = value(&json!({"Meta": {"a": "1", "B": "2"}, "Data": "d"}));
    let serialized = facade::serialize_case(protocol, &model, "Put", &input)?;
    let headers: Vec<(&str, &str)> = serialized.headers.iter().collect();
    assert!(headers.contains(&("Metaa", "1")), "{headers:?}");
    assert!(headers.contains(&("MetaB", "2")), "{headers:?}");
    assert!(
        !headers.iter().any(|(name, _)| *name == "a" || *name == "B"),
        "{headers:?}"
    );
    let response = HttpResponse {
        status: 200,
        headers: Headers::from_pairs([("x-a", "1"), ("Metab", "2")]),
        body: Vec::new(),
    };
    let parsed = facade::parse_case(
        protocol,
        &model,
        "Put",
        &response,
        &facade::botocore_options(),
    )?;
    let Value::Map(parsed) = parsed else {
        return Err("not a map".into());
    };
    assert_eq!(
        parsed.get("Meta"),
        Some(&value(&json!({"x-a": "1", "Metab": "2"})))
    );
    Ok(())
}

/// (7) A validation report lists missing members in the order of the
/// shape's `required` list, not of its members.
#[test]
fn a_validation_report_follows_required_order() -> Result {
    let (model, _) = rest()?;
    let verdict = facade::validate_case(&model, "Check", &value(&json!({"Second": "s"})))?;
    let Validated::Invalid { report, .. } = verdict else {
        return Err(format!("{verdict:?}").into());
    };
    assert_eq!(
        report,
        "Missing required parameter in input: \"Third\"\n\
         Missing required parameter in input: \"First\""
    );
    Ok(())
}

/// (8) A REST output's keys come in `BaseRestParser`'s order:
/// `ResponseMetadata`, the header members in model order, then the body's
/// members in model order, whatever order the response gives them.
#[test]
fn rest_output_keys_put_headers_before_the_body() -> Result {
    let (model, protocol) = rest()?;
    let response = HttpResponse {
        status: 200,
        headers: Headers::from_pairs([("x-two", "h2"), ("x-one", "h1")]),
        body: br#"{"BodyTwo": "b2", "BodyOne": "b1"}"#.to_vec(),
    };
    let parsed = facade::parse_case(
        protocol,
        &model,
        "Get",
        &response,
        &facade::botocore_options(),
    )?;
    assert_eq!(
        keys_of(&parsed),
        [
            "ResponseMetadata",
            "HeaderOne",
            "HeaderTwo",
            "BodyOne",
            "BodyTwo"
        ]
    );
    Ok(())
}
