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
    let operation = model.schema().service().operation("Get").ok_or("Get")?;
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
        operation,
        &Value::Map(input),
        &builtins,
        Some("123456789012"),
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

/// A built-in the rule set requires without a default, which core does not
/// supply, is missing unless a customization in scope for the service names
/// it among those it sets.
#[test]
fn a_required_built_in_needs_core_or_a_customization() -> TestResult {
    let (_, data) = fixture()?;
    assert_eq!(missing_builtin(&data, "zjson", &[]), None);
    let required = RULES.replace(
        r#""Other": {"type": "String", "builtIn": "AWS::Other::Thing", "required": false}"#,
        r#""Other": {"type": "String", "builtIn": "AWS::Other::Thing", "required": true}"#,
    );
    let data = EndpointData::from_documents(required.into_bytes(), b"{}".to_vec(), None)?;
    assert_eq!(
        missing_builtin(&data, "zjson", &[]),
        Some("AWS::Other::Thing".to_owned())
    );
    static SUPPLIER: crate::hooks::Customization = crate::hooks::Customization {
        id: "supplier",
        scope: crate::hooks::Scope::service("zjson"),
        setting_defaults: &[],
        builtins: &["AWS::Other::Thing"],
        interceptors: &[],
        replaces: &[],
    };
    assert_eq!(missing_builtin(&data, "zjson", &[&SUPPLIER]), None);
    assert_eq!(
        missing_builtin(&data, "zquery", &[&SUPPLIER]),
        Some("AWS::Other::Thing".to_owned())
    );
    Ok(())
}

/// Core supplies the constants botocore gives every client
/// (`compute_endpoint_resolver_builtin_defaults`), the STS and S3 global
/// endpoint switches off among them.
#[test]
fn core_supplies_botocore_s_constant_built_ins() -> TestResult {
    let (_, data) = fixture()?;
    let config = EndpointConfig::new(&EndpointInputs {
        region: Some("us-east-1"),
        signing: true,
        ..EndpointInputs::default()
    });
    let builtins = config.builtins(&data)?;
    for name in ["AWS::STS::UseGlobalEndpoint", "AWS::S3::UseGlobalEndpoint"] {
        assert_eq!(
            builtins.get(name),
            Some(&ParamValue::Boolean(false)),
            "{name}"
        );
    }
    assert_eq!(builtins.get("AWS::Auth::AccountId"), None);
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
        config.builtins(&global)?.get("AWS::Region"),
        Some(&ParamValue::String("aws-global".into()))
    );
    let with_endpoint = EndpointConfig::new(&EndpointInputs {
        endpoint_url: Some("http://localhost:4566"),
        ..EndpointInputs::default()
    });
    assert_eq!(with_endpoint.builtins(&global)?.get("AWS::Region"), None);
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
    cases.extend(variant_cases());
    cases
}

/// FIPS and dual-stack where `endpoints.json` may have no variant
/// (`EndpointResolver._resolve`'s `EndpointVariantError`): each iso
/// partition, the dual-stack skip of iso and iso-b, a region no partition
/// matches, a service `endpoints.json` lacks, both variants together, and
/// an endpoint URL, which does not stop the check.
fn variant_cases() -> Vec<Value> {
    let both = (Some(true), Some(true));
    let fips = (Some(true), None);
    let dual = (None, Some(true));
    let mut cases = Vec::new();
    for (service, operation) in [
        ("sts", "GetCallerIdentity"),
        ("dynamodb", "ListTables"),
        ("b2bi", "ListCapabilities"),
    ] {
        for (region, (use_fips, dualstack)) in [
            ("us-east-1", both),
            ("eu-isoe-west-1", both),
            ("eu-isoe-west-1", fips),
            ("us-iso-east-1", dual),
            ("us-iso-east-1", fips),
            ("us-isob-east-1", dual),
            ("us-isof-south-1", both),
            ("cn-north-1", fips),
            ("cn-north-1", both),
            ("us-gov-west-1", dual),
            ("eusc-de-east-1", both),
            ("mars-1", fips),
            ("mars-1", dual),
        ] {
            cases.push(case(
                service,
                operation,
                Some(region),
                use_fips,
                dualstack,
                None,
            ));
        }
    }
    cases.extend([
        case(
            "sts",
            "GetCallerIdentity",
            None,
            Some(true),
            Some(true),
            None,
        ),
        case(
            "sts",
            "GetCallerIdentity",
            Some("eu-isoe-west-1"),
            Some(true),
            Some(true),
            Some("http://localhost:4566"),
        ),
        case(
            "sts",
            "GetCallerIdentity",
            Some("fips-eu-isoe-west-1"),
            None,
            None,
            None,
        ),
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
    let data = EndpointData::load(&source, service, model.schema().service())?;
    let config = EndpointConfig::new(&EndpointInputs {
        region: text(case, "region"),
        use_fips: flag(case, "fips"),
        use_dual_stack: flag(case, "dualstack"),
        endpoint_url: text(case, "endpoint_url"),
        signing: true,
    });
    let mut result = Map::new();
    let outcome = config
        .check(&data)
        .and_then(|()| config.builtins(&data))
        .and_then(|builtins| {
            let operation = model
                .schema()
                .service()
                .operation(text(case, "operation").unwrap_or_default())
                .ok_or_else(|| CallError::Endpoint("no operation".into()))?;
            let params =
                provider_params(&data, operation, &Value::Map(Map::new()), &builtins, None);
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
            let name = match &error {
                CallError::NoRegion => "NoRegionError",
                CallError::Endpoint(text) if text.starts_with("Unable to construct a modeled") => {
                    "EndpointVariantError"
                }
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

/// `validate_region_name` (`Session.create_client`): the library refuses a
/// region that is no host label, as botocore does, though the router sends
/// such an invocation to Python first.
#[test]
fn a_region_that_is_no_host_label_is_refused() -> TestResult {
    let (_, data) = fixture()?;
    let config = EndpointConfig::new(&EndpointInputs {
        region: Some("bad region!"),
        signing: true,
        ..EndpointInputs::default()
    });
    let refused = config.check(&data).err().map(|error| error.to_string());
    assert_eq!(
        refused.as_deref(),
        Some("Provided region_name 'bad region!' doesn't match a supported format.")
    );
    Ok(())
}

/// `endpoints.json` is read only when a call needs it: no region (its
/// `partitionEndpoint`) or FIPS or dual-stack (the variant check). A call
/// with a region and neither never parses it, so a document that does not
/// parse fails only the calls that need it, and those as unsupported.
#[test]
fn endpoints_json_is_parsed_only_when_needed() -> TestResult {
    let (_, data) = fixture()?;
    let rules = data.rules.clone();
    let partitions = data.partitions.clone();
    let broken = EndpointData::from_documents(rules, partitions, Some((b"{not json", "sts")))?;
    let regional = EndpointConfig::new(&EndpointInputs {
        region: Some("us-west-2"),
        signing: true,
        ..EndpointInputs::default()
    });
    assert!(regional.check(&broken).is_ok());
    assert_eq!(
        regional.client_region(&broken)?,
        Some("us-west-2".to_owned())
    );
    let fips = EndpointConfig::new(&EndpointInputs {
        region: Some("us-west-2"),
        use_fips: Some(true),
        signing: true,
        ..EndpointInputs::default()
    });
    assert!(matches!(
        fips.check(&broken),
        Err(CallError::Unsupported(Unsupported::EndpointData))
    ));
    let none = EndpointConfig::new(&EndpointInputs {
        signing: true,
        ..EndpointInputs::default()
    });
    assert!(matches!(
        none.client_region(&broken),
        Err(CallError::Unsupported(Unsupported::EndpointData))
    ));
    Ok(())
}

/// A resolution that returns endpoint headers is refused: the client does
/// not send them, and sending without them would be a different request.
#[test]
fn an_endpoint_with_headers_is_refused() -> TestResult {
    let rules = r#"{"version": "1.0", "parameters": {
        "Region": {"type": "String", "builtIn": "AWS::Region", "required": false}},
        "rules": [{"conditions": [], "type": "endpoint", "endpoint":
            {"url": "https://zjson.example.com", "headers": {"x-amz-thing": ["on"]}}}]}"#;
    let partitions = r#"{"version": "1.1", "partitions": [{"id": "aws",
        "regionRegex": "^(us|eu)\\-\\w+\\-\\d+$", "regions": {},
        "outputs": {"name": "aws", "dnsSuffix": "example.com", "dualStackDnsSuffix": "example.com",
        "supportsFIPS": true, "supportsDualStack": true, "implicitGlobalRegion": "us-east-1"}}]}"#;
    let data = EndpointData::from_documents(
        rules.as_bytes().to_vec(),
        partitions.as_bytes().to_vec(),
        None,
    )?;
    let mut params = EndpointParams::default();
    params.set("Region", ParamValue::String("us-west-2".into()));
    assert_eq!(
        resolve(&data, &params).err(),
        Some(CallError::Unsupported(Unsupported::EndpointHeaders))
    );
    Ok(())
}

/// Every endpoint rule under `rules`, with whether a condition on the way
/// to it reads `AccountId`.
fn endpoint_rules<'a>(rules: &'a Value, under: bool, out: &mut Vec<(bool, &'a Value)>) {
    let Value::List(rules) = rules else {
        return;
    };
    for rule in rules {
        let conditions = field(rule, "conditions").map(model::pyrepr::py_repr);
        let reads = under || conditions.is_some_and(|text| text.contains("'AccountId'"));
        match text(rule, "type") {
            Some("tree") => {
                endpoint_rules(field(rule, "rules").unwrap_or(&Value::Null), reads, out)
            }
            Some("endpoint") => out.push((reads, rule)),
            _ => {}
        }
    }
}

/// Every shipped rule set, by its path.
fn shipped_rule_sets() -> TestResult<Vec<(std::path::PathBuf, Value)>> {
    let mut found = Vec::new();
    for service in std::fs::read_dir(repository().join("awscli/botocore/data"))? {
        let service = service?.path();
        if !service.is_dir() {
            continue;
        }
        for version in std::fs::read_dir(&service)? {
            let path = version?.path().join("endpoint-rule-set-1.json");
            if let Ok(bytes) = std::fs::read(&path) {
                found.push((path, Value::from_json(&bytes)?));
            }
        }
    }
    Ok(found)
}

/// How many endpoints of `rules` an `AccountId` condition leads to, each
/// checked to carry no headers and to sign with SigV4 or nothing.
fn check_account_endpoints(path: &std::path::Path, rules: &Value) -> TestResult<usize> {
    let mut endpoints = Vec::new();
    endpoint_rules(
        field(rules, "rules").unwrap_or(&Value::Null),
        false,
        &mut endpoints,
    );
    let account = endpoints.iter().filter(|(reads, _)| *reads);
    let mut count = 0;
    for (_, rule) in account {
        let endpoint = field(rule, "endpoint").ok_or("endpoint")?;
        let headers =
            matches!(field(endpoint, "headers"), Some(Value::Map(map)) if !map.is_empty());
        assert!(!headers, "{}", path.display());
        let schemes = field(endpoint, "properties").and_then(|props| field(props, "authSchemes"));
        if let Some(Value::List(schemes)) = schemes {
            let names: Vec<Option<&str>> =
                schemes.iter().map(|scheme| text(scheme, "name")).collect();
            assert!(
                names.iter().all(|name| *name == Some("sigv4")),
                "{}",
                path.display()
            );
        }
        count += 1;
    }
    Ok(count)
}

/// The deferred account id resolves only in `send`, after credentials, where
/// a refusal can no longer send the call to Python. So no shipped rule set
/// may give an account-id endpoint a refusal its other endpoints lack: none
/// carries headers, and each signs with SigV4 or nothing. Prints the
/// rule sets it checked.
#[test]
fn account_id_endpoints_refuse_nothing_more() -> TestResult {
    let account = |def: &Value| text(def, "builtIn") == Some("AWS::Auth::AccountId");
    let mut checked = String::from("account-id endpoints checked:\n");
    for (path, rules) in shipped_rule_sets()? {
        let reads = matches!(field(&rules, "parameters"), Some(Value::Map(parameters))
            if parameters.iter().any(|(_, def)| account(def)));
        if reads {
            let count = check_account_endpoints(&path, &rules)?;
            checked.push_str(&format!("{} {count}\n", path.display()));
        }
    }
    assert!(checked.lines().count() > 1);
    std::io::Write::write_all(&mut std::io::stderr(), checked.as_bytes())?;
    Ok(())
}

/// `UnknownSignatureVersionError` names the schemes as botocore has
/// rebound them, without their `sig` prefix (`auth_schemes_to_signing_ctx`
/// strips them before it builds `auth_type_options`).
#[test]
fn an_unknown_scheme_is_named_without_its_sig_prefix() {
    let scheme = |name: &str| SchemeProps {
        name: name.into(),
        ..SchemeProps::default()
    };
    let endpoint = ResolvedEndpoint {
        url: "https://zjson.example.com".into(),
        auth_schemes: vec![scheme("sigv5"), scheme("custom")],
    };
    assert_eq!(
        endpoint.signing_context().err().as_deref(),
        Some("Unknown Signature Version: v5, custom.")
    );
}

const CONTEXT_RULES: &str = r#"{"version": "1.0", "parameters": {
    "Region": {"type": "String", "builtIn": "AWS::Region", "required": false},
    "ServiceType": {"type": "String", "required": true}},
    "rules": [{"conditions": [], "error": "unused", "type": "error"}]}"#;

const CONTEXT_SERVICE: &str = r#"{"metadata": {"protocol": "json", "jsonVersion": "1.1"},
    "clientContextParams": {"ServiceType": {"type": "string"}},
    "operations": {
        "Pinned": {"name": "Pinned", "http": {"method": "POST", "requestUri": "/"},
            "staticContextParams": {"ServiceType": {"value": "ACM"}}},
        "Loose": {"name": "Loose", "http": {"method": "POST", "requestUri": "/"}}
    },
    "shapes": {}}"#;

/// `_resolve_param_as_client_context_param` comes after the operation's
/// context parameters and before the built-ins, and the CLI's client
/// context is empty: a required client-context parameter is supplied only
/// by an operation that sets it, and an operation that does not is refused
/// before any call (acm's `ServiceType` is set statically by every
/// operation).
#[test]
fn a_required_client_context_parameter_needs_the_operation() -> TestResult {
    let model = Model::from_json(CONTEXT_SERVICE.as_bytes())?;
    let data =
        EndpointData::from_documents(CONTEXT_RULES.as_bytes().to_vec(), b"{}".to_vec(), None)?;
    let pinned = model
        .schema()
        .service()
        .operation("Pinned")
        .ok_or("Pinned")?;
    let loose = model.schema().service().operation("Loose").ok_or("Loose")?;
    assert_eq!(missing_client_context(&data, &model, pinned), None);
    assert_eq!(
        missing_client_context(&data, &model, loose),
        Some("ServiceType".to_owned())
    );
    let builtins = Builtins::default();
    let params = provider_params(&data, loose, &Value::Null, &builtins, None);
    assert_eq!(params.get("ServiceType"), None);
    let params = provider_params(&data, pinned, &Value::Null, &builtins, None);
    assert_eq!(
        params.get("ServiceType"),
        Some(&ParamValue::String("ACM".into()))
    );
    Ok(())
}
