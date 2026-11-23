use super::*;
use crate::error::CallError;
use model::shapes::Model;
use model::source::DirSource;
use model::value::{Map, Value};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// `ClientCreator._normalize_fips_region`: `fips-` and `-fips` go, wherever
/// they are, and FIPS is then on.
#[test]
fn fips_pseudo_regions_normalize_and_turn_fips_on() {
    for (region, normalized, pseudo) in [
        ("us-east-1", "us-east-1", false),
        ("fips-us-east-1", "us-east-1", true),
        ("us-east-1-fips", "us-east-1", true),
        ("fips-us-gov-west-1-fips", "us-gov-west-1", true),
        ("aws-global", "aws-global", false),
    ] {
        assert_eq!(
            normalize_fips_region(region),
            (normalized.to_owned(), pseudo),
            "{region}"
        );
    }
    let config = EndpointConfig::new(&EndpointInputs {
        region: Some("fips-us-east-1"),
        use_fips: Some(false),
        ..EndpointInputs::default()
    });
    assert_eq!(config.region(), Some("us-east-1"));
    assert!(config.use_fips());
}

/// `compute_endpoint_resolver_builtin_defaults`: FIPS and dual-stack are
/// off with a configured endpoint, which is `SDK::Endpoint`; account ids
/// route as `preferred` unless the client does not sign.
#[test]
fn builtins_follow_botocore_s_defaults() {
    let config = EndpointConfig::new(&EndpointInputs {
        region: Some("us-west-2"),
        use_fips: Some(true),
        use_dual_stack: Some(true),
        endpoint_url: Some("http://localhost:4566"),
        signing: true,
    });
    assert!(!config.use_fips() && !config.use_dual_stack());
    assert_eq!(config.endpoint_url(), Some("http://localhost:4566"));
    assert_eq!(config.account_id_endpoint_mode(), "preferred");
    let unsigned = EndpointConfig::new(&EndpointInputs {
        region: Some("us-west-2"),
        signing: false,
        ..EndpointInputs::default()
    });
    assert_eq!(unsigned.account_id_endpoint_mode(), "disabled");
}

/// A rule set whose parameters come from each of the sources botocore
/// reads, in its order: static context, an input member's context
/// parameter, then built-ins; an unset one is left out.
const RULES: &str = r#"{
    "version": "1.0",
    "parameters": {
        "Region": {"type": "String", "builtIn": "AWS::Region", "required": false},
        "UseFIPS": {"type": "Boolean", "builtIn": "AWS::UseFIPS", "required": true,
            "default": false},
        "Endpoint": {"type": "String", "builtIn": "SDK::Endpoint", "required": false},
        "Static": {"type": "Boolean", "required": false},
        "TableName": {"type": "String", "required": false},
        "Other": {"type": "String", "builtIn": "AWS::Other::Thing", "required": false},
        "AccountId": {"type": "String", "builtIn": "AWS::Auth::AccountId", "required": false},
        "AccountIdEndpointMode": {"type": "String",
            "builtIn": "AWS::Auth::AccountIdEndpointMode", "required": false}
    },
    "rules": [{"conditions": [], "endpoint": {"url": "https://example.com"}, "type": "endpoint"}]
}"#;

const SERVICE: &str = r#"{
    "metadata": {"protocol": "json", "endpointPrefix": "zjson"},
    "operations": {
        "Get": {"name": "Get", "http": {"method": "POST", "requestUri": "/"},
            "input": {"shape": "GetInput"},
            "staticContextParams": {"Static": {"value": true}}},
        "Find": {"name": "Find", "http": {"method": "POST", "requestUri": "/"},
            "operationContextParams": {"Names": {"path": "Items[*].Name"}}}
    },
    "shapes": {
        "GetInput": {"type": "structure", "members": {
            "Table": {"shape": "S", "contextParam": {"name": "TableName"}}}},
        "S": {"type": "string"}
    }
}"#;

fn fixture() -> TestResult<(Model, EndpointData)> {
    let model = Model::from_json(SERVICE.as_bytes())?;
    let data = EndpointData::from_documents(RULES.as_bytes().to_vec(), b"{}".to_vec(), None)?;
    Ok((model, data))
}

#[test]
fn parameters_come_from_each_source_in_rule_set_order() -> TestResult {
    let (model, data) = fixture()?;
    let operation = model.operation("Get").ok_or("Get")?;
    let mut input = Map::new();
    input.insert("Table".into(), Value::String("orders".into()));
    let config = EndpointConfig::new(&EndpointInputs {
        region: Some("eu-west-1"),
        signing: true,
        ..EndpointInputs::default()
    });
    let builtins = config.builtins(&data)?;
    let params = provider_params(
        &data,
        &model,
        operation,
        &Value::Map(input),
        &builtins,
        || Some("123456789012".into()),
    );
    let named: Vec<(&str, &ParamValue)> = params.iter().collect();
    let string = |text: &str| ParamValue::String(text.into());
    assert_eq!(
        named,
        [
            ("Region", &string("eu-west-1")),
            ("UseFIPS", &ParamValue::Boolean(false)),
            ("Static", &ParamValue::Boolean(true)),
            ("TableName", &string("orders")),
            ("AccountId", &string("123456789012")),
            ("AccountIdEndpointMode", &string("preferred")),
        ]
    );
    Ok(())
}

/// What admission refuses: operation context parameters (JMESPath, not
/// implemented), and a built-in core does not know that the rule set
/// requires without a default, unless an interceptor set it.
#[test]
fn admission_refuses_what_core_cannot_supply() -> TestResult {
    let (model, data) = fixture()?;
    let find = model.operation("Find").ok_or("Find")?;
    let get = model.operation("Get").ok_or("Get")?;
    let empty = EndpointParams::default();
    assert_eq!(
        unsupported(&data, find, &empty),
        Some(Unsupported::OperationContextParams)
    );
    assert_eq!(unsupported(&data, get, &empty), None);
    let required = RULES.replace(
        r#""Other": {"type": "String", "builtIn": "AWS::Other::Thing", "required": false}"#,
        r#""Other": {"type": "String", "builtIn": "AWS::Other::Thing", "required": true}"#,
    );
    let data = EndpointData::from_documents(required.into_bytes(), b"{}".to_vec(), None)?;
    assert_eq!(
        unsupported(&data, get, &empty),
        Some(Unsupported::Builtin("AWS::Other::Thing".into()))
    );
    let mut supplied = EndpointParams::default();
    supplied.set("Other", ParamValue::String("x".into()));
    assert_eq!(unsupported(&data, get, &supplied), None);
    Ok(())
}

/// With no region, botocore's legacy resolver raises `NoRegionError` unless
/// the service has a partition endpoint, which is then the client's region,
/// unless an endpoint is given.
#[test]
fn no_region_needs_a_partition_endpoint() -> TestResult {
    let endpoints = |service: &str| {
        format!(
            r#"{{"partitions": [{{"partition": "aws", "services": {{
                "{service}": {{"partitionEndpoint": "aws-global", "endpoints": {{}}}}}}}}]}}"#
        )
    };
    let config = EndpointConfig::new(&EndpointInputs::default());
    let global = EndpointData::from_documents(
        RULES.as_bytes().to_vec(),
        b"{}".to_vec(),
        Some((endpoints("zjson").as_bytes(), "zjson")),
    )?;
    assert_eq!(
        config.builtins(&global)?.region.as_deref(),
        Some("aws-global")
    );
    let with_endpoint = EndpointConfig::new(&EndpointInputs {
        endpoint_url: Some("http://localhost:4566"),
        ..EndpointInputs::default()
    });
    assert_eq!(with_endpoint.builtins(&global)?.region, None);
    let regional = EndpointData::from_documents(
        RULES.as_bytes().to_vec(),
        b"{}".to_vec(),
        Some((endpoints("other").as_bytes(), "zjson")),
    )?;
    assert!(matches!(
        config.builtins(&regional),
        Err(CallError::NoRegion)
    ));
    Ok(())
}

/// One case of the parity table: an operation of a service, resolved with
/// these settings.
fn case(
    service: &str,
    operation: &str,
    region: Option<&str>,
    fips: Option<bool>,
    dualstack: Option<bool>,
    endpoint_url: Option<&str>,
) -> Value {
    let mut map = Map::new();
    let text = |value: Option<&str>| value.map_or(Value::Null, |v| Value::String(v.into()));
    let flag = |value: Option<bool>| value.map_or(Value::Null, Value::Bool);
    map.insert("service".into(), Value::String(service.into()));
    map.insert("operation".into(), Value::String(operation.into()));
    map.insert("region".into(), text(region));
    map.insert("fips".into(), flag(fips));
    map.insert("dualstack".into(), flag(dualstack));
    map.insert("endpoint_url".into(), text(endpoint_url));
    Value::Map(map)
}

fn cases() -> Vec<Value> {
    let sts = |region, fips, dualstack, url| {
        case("sts", "GetCallerIdentity", region, fips, dualstack, url)
    };
    let ddb = |region, fips, url| case("dynamodb", "ListTables", region, fips, None, url);
    let iam = |region, fips| case("iam", "ListUsers", region, fips, None, None);
    let mut cases = Vec::new();
    for region in [
        "us-east-1",
        "us-west-2",
        "eu-west-1",
        "cn-north-1",
        "us-gov-west-1",
        "aws-global",
        "fips-us-east-1",
        "us-east-1-fips",
        "us-east-9",
        "mars-1",
        "us-iso-east-1",
        "eu-isoe-west-1",
    ] {
        cases.push(sts(Some(region), None, None, None));
    }
    cases.extend([
        sts(None, None, None, None),
        sts(Some("us-west-2"), Some(true), None, None),
        sts(Some("us-west-2"), None, Some(true), None),
        sts(Some("us-west-2"), Some(true), Some(true), None),
        sts(Some("cn-north-1"), None, Some(true), None),
        sts(
            Some("us-west-2"),
            None,
            None,
            Some("https://sts.example.com/base"),
        ),
        sts(
            Some("us-west-2"),
            Some(true),
            None,
            Some("http://localhost:4566"),
        ),
        sts(None, None, None, Some("http://localhost:4566")),
        ddb(Some("us-east-1"), None, None),
        ddb(None, None, None),
        ddb(None, None, Some("http://localhost:8000")),
        ddb(Some("cn-north-1"), None, None),
        ddb(Some("us-gov-west-1"), Some(true), None),
        ddb(Some("fips-us-west-2"), None, None),
        iam(Some("us-west-2"), None),
        iam(None, None),
        iam(Some("aws-global"), None),
        iam(Some("cn-north-1"), None),
        iam(Some("us-gov-west-1"), None),
        iam(Some("us-east-1"), Some(true)),
    ]);
    cases
}

fn repository() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../aws-cli")
}

/// botocore's results for `cases`, from `oracle.py` in the toolchain's
/// CPython.
fn botocore(cases: &[Value]) -> TestResult<Vec<Value>> {
    let prefix = std::env::var_os("AWS_NATIVE_CPYTHON_PREFIX")
        .ok_or("AWS_NATIVE_CPYTHON_PREFIX must name the toolchain CPython")?;
    let oracle = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/endpoint/oracle.py");
    let mut child =
        std::process::Command::new(std::path::PathBuf::from(prefix).join("bin/python3"))
            .arg("-I")
            .arg(oracle)
            .arg(repository().join("awscli").parent().ok_or("parent")?)
            .env_clear()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
    let input = model::pyjson::dumps(
        &Value::List(cases.to_vec()),
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

fn field<'a>(case: &'a Value, name: &str) -> Option<&'a Value> {
    match case {
        Value::Map(map) => map.get(name).filter(|value| **value != Value::Null),
        _ => None,
    }
}

fn text<'a>(case: &'a Value, name: &str) -> Option<&'a str> {
    match field(case, name) {
        Some(Value::String(text)) => Some(text),
        _ => None,
    }
}

fn flag(case: &Value, name: &str) -> Option<bool> {
    match field(case, name) {
        Some(Value::Bool(value)) => Some(*value),
        _ => None,
    }
}

/// The native result of `case`, in the oracle's shape.
fn native(case: &Value) -> TestResult<Value> {
    let source = DirSource::new(repository().join("awscli/botocore/data"));
    let service = text(case, "service").ok_or("service")?;
    let model = Model::load(&source, service)?;
    let data = EndpointData::load(&source, service, model.metadata())?;
    let config = EndpointConfig::new(&EndpointInputs {
        region: text(case, "region"),
        use_fips: flag(case, "fips"),
        use_dual_stack: flag(case, "dualstack"),
        endpoint_url: text(case, "endpoint_url"),
        signing: true,
    });
    let mut result = Map::new();
    let outcome = config.builtins(&data).and_then(|builtins| {
        let operation = model
            .operation(text(case, "operation").unwrap_or_default())
            .ok_or_else(|| CallError::Endpoint("no operation".into()))?;
        let params = provider_params(
            &data,
            &model,
            operation,
            &Value::Map(Map::new()),
            &builtins,
            || None,
        );
        resolve(&data, &params)
    });
    match outcome {
        Ok(endpoint) => {
            let signing = endpoint.signing_context().map_err(|e| e.to_string())?;
            let opt = |value: Option<String>| value.map_or(Value::Null, Value::String);
            result.insert("url".into(), Value::String(endpoint.url));
            result.insert(
                "signing_region".into(),
                opt(signing.as_ref().and_then(|s| s.region.clone())),
            );
            result.insert("signing_name".into(), opt(signing.and_then(|s| s.name)));
        }
        Err(error) => {
            let name = match error {
                CallError::NoRegion => "NoRegionError",
                _ => "EndpointResolutionError",
            };
            result.insert("error".into(), Value::String(name.into()));
            result.insert("message".into(), Value::String(error.to_string()));
        }
    }
    Ok(Value::Map(result))
}

/// Each case resolves to the URL and signing context botocore resolves,
/// or fails with its error and text: sts across partitions, with FIPS,
/// dual-stack, a configured endpoint, a FIPS pseudo-region, `aws-global`,
/// a region `endpoints.json` lacks, one no partition matches, and none;
/// dynamodb with and without a region; iam's global endpoint.
#[test]
#[cfg_attr(miri, ignore = "runs CPython and the CRT, which Miri cannot")]
fn endpoints_resolve_as_botocore_resolves_them() -> TestResult {
    let cases = cases();
    let expected = botocore(&cases)?;
    assert_eq!(expected.len(), cases.len());
    for (case, expected) in cases.iter().zip(&expected) {
        assert_eq!(&native(case)?, expected, "{case:?}");
    }
    Ok(())
}
