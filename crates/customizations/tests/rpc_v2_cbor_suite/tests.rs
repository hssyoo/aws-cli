//! The smithy-rpc-v2-cbor suites' replay, the decode vectors and the
//! fakes' comparison; see the crate file's docs.
use client::codec::cbor::{Item, decode};
use client::error::ParseError;
use client::http::{Headers, HttpResponse};
use client::testing::facade::{
    Model, ProtocolHandle, botocore_timestamp, built_protocol_named, model_from_json, parse_case,
    parse_error_case, parse_options, serialize_case,
};
use model::value::{Key, Map, Value};
use serde_json::Value as JsonDoc;
use std::path::PathBuf;
use std::sync::Arc;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const PROTOCOL: &str = "smithy-rpc-v2-cbor";
const FILES: [&str; 3] = [
    "smithy-rpc-v2-cbor",
    "smithy-rpc-v2-cbor-query-compatible",
    "smithy-rpc-v2-cbor-non-query-compatible",
];

fn unit_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../aws-cli/tests/unit/botocore")
}

fn read(path: PathBuf) -> Result<JsonDoc> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

fn protocol() -> Result<ProtocolHandle> {
    Ok(built_protocol_named(PROTOCOL).ok_or("smithy-rpc-v2-cbor is not built")?)
}

/// `_should_ignore_test`, under the file's own name.
fn ignored(list: &JsonDoc, file: &str, kind: &str, suite: &str, id: Option<&str>) -> bool {
    let listed = |section: &JsonDoc| {
        let names = |key: &str| section[key].as_array().cloned().unwrap_or_default();
        names("suites")
            .iter()
            .any(|name| name.as_str() == Some(suite))
            || id.is_some_and(|id| names("cases").iter().any(|name| name.as_str() == Some(id)))
    };
    listed(&list["general"][kind]) || listed(&list["protocols"][file][kind])
}

/// Python's `==` over parsed values: dicts ignore order, numbers compare by
/// value across `int`, `float` and `bool`, `nan` equals nothing.
fn python_eq(a: &Value, b: &Value) -> bool {
    match (number(a), number(b)) {
        (Some(x), Some(y)) => return x == y,
        (Some(_), None) | (None, Some(_)) => return false,
        (None, None) => {}
    }
    match (a, b) {
        (Value::Map(x), Value::Map(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get_key(k).is_some_and(|w| python_eq(v, w)))
        }
        (Value::List(x), Value::List(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| python_eq(v, w))
        }
        _ => a == b,
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Bool(flag) => Some(f64::from(u8::from(*flag))),
        Value::Integer(n) => Some(*n as f64),
        Value::BigInteger(text) => text.parse().ok(),
        Value::Float(n) => Some(*n),
        _ => None,
    }
}

/// The exact comparison of the fakes: keys in order, `nan` equal to `nan`.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x == y || (x.is_nan() && y.is_nan()),
        (Value::Map(x), Value::Map(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y.iter())
                    .all(|((k, v), (l, w))| k == l && same(v, w))
        }
        (Value::List(x), Value::List(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same(v, w))
        }
        _ => a == b,
    }
}

/// One case's model: the suite's description with the case as its only
/// operation, under `given.name` or `OperationName`.
fn case_model(suite: &JsonDoc, case: &JsonDoc) -> Result<(Arc<Model>, String)> {
    let name = (case["given"]["name"].as_str())
        .unwrap_or("OperationName")
        .to_owned();
    let mut description = suite.clone();
    let object = description.as_object_mut().ok_or("suite")?;
    object.remove("cases");
    let mut operations = serde_json::Map::new();
    operations.insert(name.clone(), case["given"].clone());
    object.insert("operations".into(), JsonDoc::Object(operations));
    Ok((model_from_json(&serde_json::to_vec(&description)?)?, name))
}

/// `_convert_strings_to_special_float` (`out` false) or
/// `_convert_special_floats_to_string` (`out` true), on the top level.
fn special_floats(value: Value, out: bool) -> Value {
    let Value::Map(map) = value else {
        return value;
    };
    let converted = map.into_iter().map(|(key, value)| {
        let value = match (&value, out) {
            (Value::String(text), false) => match text.as_str() {
                "Infinity" => Value::Float(f64::INFINITY),
                "-Infinity" => Value::Float(f64::NEG_INFINITY),
                "NaN" => Value::Float(f64::NAN),
                _ => value,
            },
            (Value::Float(n), true) if n.is_nan() => Value::String("NaN".into()),
            (Value::Float(n), true) if n.is_infinite() => {
                Value::String(if *n > 0.0 { "Infinity" } else { "-Infinity" }.into())
            }
            _ => value,
        };
        (key, value)
    });
    Value::Map(converted.collect())
}

/// `_convert_bytes_to_str`.
fn bytes_to_text(value: Value) -> Value {
    match value {
        Value::Blob(bytes) => Value::String(String::from_utf8_lossy(&bytes).into_owned()),
        Value::Map(map) => Value::Map(
            map.into_iter()
                .map(|(k, v)| (k, bytes_to_text(v)))
                .collect(),
        ),
        Value::List(items) => Value::List(items.into_iter().map(bytes_to_text).collect()),
        other => other,
    }
}

/// `parser.parse_data_item` of a body with botocore's default parser.
fn decoded(body: &[u8]) -> Option<Value> {
    decode(body, &botocore_timestamp).ok()?.to_value().ok()
}

/// `_assert_requests_equal`'s body check for CBOR: both decoded, the top
/// level's special floats as text, then `==`; an empty actual body against
/// the expected text as bytes.
fn same_body(actual: &[u8], expected: &str) -> bool {
    if actual.is_empty() {
        return expected.is_empty();
    }
    let Ok(expected) = pytext::base64::decode(expected) else {
        return false;
    };
    match (decoded(actual), decoded(&expected)) {
        (Some(a), Some(e)) => python_eq(&special_floats(a, true), &special_floats(e, true)),
        _ => false,
    }
}

/// `_assert_expected_headers_in_request`: every expected header present
/// with its value, and no forbidden one.
fn check_headers(actual: &Headers, expected: &JsonDoc) -> Result<()> {
    for (name, value) in expected["headers"].as_object().into_iter().flatten() {
        let (got, value) = (actual.get(name), value.as_str());
        if got != value {
            return Err(format!("header {name}: {got:?} != {value:?}").into());
        }
    }
    for name in expected["forbidHeaders"].as_array().into_iter().flatten() {
        if actual.contains(name.as_str().unwrap_or_default()) {
            return Err(format!("forbidden header {name}").into());
        }
    }
    Ok(())
}

/// `test_input_compliance` for one case.
fn input_case(suite: &JsonDoc, case: &JsonDoc) -> Result<()> {
    let (model, name) = case_model(suite, case)?;
    let params = special_floats(Value::from_serde(&case["params"]), false);
    let request = serialize_case(protocol()?, &model, &name, &params)?;
    let expected = &case["serialized"];
    let body = expected["body"].as_str().unwrap_or("");
    if (!request.body.is_empty() || expected.get("body").is_some())
        && !same_body(&request.body, body)
    {
        return Err(format!(
            "body {:?} != {body:?}",
            pytext::base64::encode(&request.body)
        )
        .into());
    }
    check_headers(&request.headers, expected)?;
    let uri = expected["uri"].as_str().unwrap_or("");
    if request.path != uri || !request.query.is_empty() {
        return Err(format!("uri {}?{} != {uri}", request.path, request.query).into());
    }
    match expected["method"].as_str() {
        Some(method) if method != request.method.as_str() => {
            Err(format!("method {} != {method}", request.method.as_str()).into())
        }
        _ if expected.get("host").is_some() || request.host_prefix.is_some() => {
            Err("a host prefix the suite does not have".into())
        }
        _ => Ok(()),
    }
}

/// `_compliance_timestamp_parser`: seconds since the epoch, as a float.
fn compliance_timestamp(value: &Value) -> std::result::Result<Value, ParseError> {
    match botocore_timestamp(value)? {
        Value::Timestamp(at) => Ok(Value::Float(at.unix_micros() as f64 / 1e6)),
        other => Ok(other),
    }
}

fn case_response(raw: &JsonDoc) -> Result<HttpResponse> {
    let pairs = raw["headers"].as_object().into_iter().flatten();
    let headers = Headers::from_pairs(pairs.map(|(k, v)| (k.as_str(), v.as_str().unwrap_or(""))));
    let status = raw["status_code"]
        .as_u64()
        .and_then(|s| u16::try_from(s).ok());
    let body = pytext::base64::decode(raw["body"].as_str().unwrap_or(""))?;
    Ok(HttpResponse {
        status: status.unwrap_or(200),
        headers,
        body,
    })
}

/// `test_output_compliance` for one case, after `_fixup_parsed_result`.
fn output_case(suite: &JsonDoc, case: &JsonDoc) -> Result<()> {
    let (model, name) = case_model(suite, case)?;
    let response = case_response(&case["response"])?;
    let keep = |value: &Value| Ok(value.clone());
    let options = parse_options(&compliance_timestamp, &keep);
    let (parsed, expected) = if case.get("error").is_some() {
        let error = parse_error_case(protocol()?, &model, &name, &response, &options)?;
        let mut parsed = Map::new();
        let kept = (error.error.iter())
            .filter(|(k, _)| matches!(k.as_str(), Some("Code" | "Message")))
            .map(|(k, v)| (k.clone(), v.clone()));
        parsed.insert("Error".into(), Value::Map(kept.collect()));
        for (key, value) in error.members {
            parsed.insert_key(key, value);
        }
        let field = |key: &str| Value::String(case[key].as_str().unwrap_or("").to_owned());
        let mut wanted = Map::new();
        let mut error = Map::new();
        error.insert("Code".into(), field("errorCode"));
        error.insert("Message".into(), field("errorMessage"));
        wanted.insert("Error".into(), Value::Map(error));
        if let Value::Map(members) = Value::from_serde(&case["error"]) {
            for (key, value) in members {
                wanted.insert_key(key, value);
            }
        }
        (parsed, Value::Map(wanted))
    } else {
        let Value::Map(mut parsed) = parse_case(protocol()?, &model, &name, &response, &options)?
        else {
            return Err("not a structure".into());
        };
        parsed.remove("ResponseMetadata");
        (parsed, Value::from_serde(&case["result"]))
    };
    let parsed = special_floats(bytes_to_text(Value::Map(parsed)), true);
    if python_eq(&parsed, &expected) {
        Ok(())
    } else {
        Err(format!("parsed {parsed:?}\nexpected {expected:?}").into())
    }
}

/// The counts of a run: passed, ignored, ignored but passing, and each
/// failure.
#[derive(Default)]
struct Tally {
    passed: usize,
    ignored: usize,
    ignored_passing: usize,
    failed: Vec<String>,
}

/// Every `kind` case of one file, counted into `tally`.
fn run_file(list: &JsonDoc, kind: &str, file: &str, tally: &mut Tally) -> Result<()> {
    let key = if kind == "input" {
        "params"
    } else {
        "response"
    };
    let suites = read(unit_dir().join(format!("protocols/{kind}/{file}.json")))?;
    for (suite_index, suite) in suites.as_array().ok_or("suites")?.iter().enumerate() {
        let description = suite["description"].as_str().unwrap_or_default();
        let cases = suite["cases"].as_array().into_iter().flatten().enumerate();
        for (index, case) in cases.filter(|(_, case)| case.get(key).is_some()) {
            let outcome = match kind {
                "input" => input_case(suite, case),
                _ => output_case(suite, case),
            };
            let id = case["id"].as_str();
            if ignored(list, file, kind, description, id) {
                tally.ignored += 1;
                tally.ignored_passing += usize::from(outcome.is_ok());
            } else if let Err(error) = outcome {
                let id = id.unwrap_or("");
                (tally.failed).push(format!(
                    "{kind}/{file} {description} ({suite_index}:{index}) {id}: {error}"
                ));
            } else {
                tally.passed += 1;
            }
        }
    }
    Ok(())
}

/// Every `kind` case of the three files: a report of the counts on stderr,
/// and a failure naming each case that failed.
fn run_suite(kind: &str) -> Result<()> {
    let list = read(unit_dir().join("protocols/protocol-tests-ignore-list.json"))?;
    let mut tally = Tally::default();
    for file in FILES {
        run_file(&list, kind, file, &mut tally)?;
    }
    let Tally {
        passed,
        ignored: ignored_cases,
        ignored_passing,
        failed,
    } = tally;
    let report = format!(
        "{kind}/{PROTOCOL}*: passed {passed} failed {} ignored {ignored_cases} \
         ({ignored_passing} passing)\n",
        failed.len()
    );
    std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes())?;
    assert!(passed > 0, "no {kind} case ran");
    assert!(
        failed.is_empty(),
        "{} failed:\n{}",
        failed.len(),
        failed.join("\n")
    );
    Ok(())
}

#[test]
fn botocore_s_rpc_v2_cbor_input_suites_pass() -> Result<()> {
    run_suite("input")
}

#[test]
fn botocore_s_rpc_v2_cbor_output_suites_pass() -> Result<()> {
    run_suite("output")
}

/// `test_cbor_decoding.py`'s `IGNORE_CASES`.
const DECODE_IGNORED: [&str; 12] = [
    "tag - 0/min",
    "tag - 1/min",
    "tag - 2/min",
    "tag - 4/min",
    "tag - 8/min",
    "tag - 0/max",
    "tag - 1/max",
    "tag - 2/max",
    "tag - 4/max",
    "tag - 8/max",
    "map - {_ null}",
    "map - {null}",
];

/// `_assert_expected_value`: each expected key's assertion; a list's and a
/// map's own values only as far as the expectation lists them.
fn expected_value(actual: &Item, expected: &JsonDoc) -> bool {
    let Some(object) = expected.as_object() else {
        return false;
    };
    object
        .iter()
        .all(|(key, value)| match (key.as_str(), actual) {
            ("null" | "undefined", _) => *actual == Item::Scalar(Value::Null),
            ("bytestring", Item::Scalar(Value::Blob(bytes))) => {
                let wanted: Vec<u8> = (value.as_array().into_iter().flatten())
                    .filter_map(|byte| byte.as_u64().and_then(|b| u8::try_from(b).ok()))
                    .collect();
                *bytes == wanted
            }
            ("list", Item::List(items)) => (items.iter())
                .zip(value.as_array().into_iter().flatten())
                .all(|(item, expected)| expected_value(item, expected)),
            ("map", Item::Map(_)) => {
                (value.as_object().into_iter().flatten()).all(|(key, expected)| {
                    actual
                        .get(key)
                        .is_some_and(|item| expected_value(item, expected))
                })
            }
            ("float32", Item::Scalar(Value::Float(n))) => {
                let bits = value.as_u64().and_then(|b| u32::try_from(b).ok());
                bits.is_some_and(|bits| f64::from(f32::from_bits(bits)) == *n)
            }
            ("float64", Item::Scalar(Value::Float(n))) => {
                value.as_u64().is_some_and(|b| f64::from_bits(b) == *n)
            }
            (_, Item::Scalar(scalar)) => python_eq(scalar, &Value::from_serde(value)),
            _ => false,
        })
}

fn from_hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .filter_map(|at| u8::from_str_radix(&text[2 * at..2 * at + 2], 16).ok())
        .collect()
}

fn keep(value: &Value) -> std::result::Result<Value, ParseError> {
    Ok(value.clone())
}

#[test]
fn botocore_s_cbor_decode_vectors_pass() -> Result<()> {
    let success = read(unit_dir().join("cbor/decode-success-tests.json"))?;
    let mut failed = Vec::new();
    let mut ran = 0;
    for case in success.as_array().ok_or("success")? {
        let description = case["description"].as_str().unwrap_or_default();
        if DECODE_IGNORED.contains(&description) {
            continue;
        }
        ran += 1;
        match decode(&from_hex(case["input"].as_str().unwrap_or("")), &keep) {
            Ok(item) if expected_value(&item, &case["expect"]) => {}
            other => failed.push(format!("{description}: {other:?}")),
        }
    }
    let errors = read(unit_dir().join("cbor/decode-error-tests.json"))?;
    for case in errors.as_array().ok_or("errors")? {
        ran += 1;
        let description = case["description"].as_str().unwrap_or_default();
        if let Ok(item) = decode(&from_hex(case["input"].as_str().unwrap_or("")), &keep) {
            failed.push(format!("{description}: decoded {item:?}"));
        }
    }
    assert!(ran > 190, "only {ran} vectors");
    assert!(
        failed.is_empty(),
        "{} failed:\n{}",
        failed.len(),
        failed.join("\n")
    );
    Ok(())
}

/// A value as `fakes.json` spells it: `{"$blob": base64}` is bytes,
/// `{"$float": repr}` a float JSON cannot hold, `{"$int": text}` an int
/// past 64 bits.
fn fake(json: &JsonDoc) -> Result<Value> {
    let one = |key: &str| {
        json.as_object()
            .filter(|o| o.len() == 1)
            .and_then(|o| o.get(key)?.as_str())
    };
    if let Some(text) = one("$blob") {
        return Ok(Value::Blob(pytext::base64::decode(text)?));
    }
    if let Some(text) = one("$float") {
        return Ok(Value::Float(text.parse()?));
    }
    if let Some(text) = one("$int") {
        return Ok(text
            .parse::<i64>()
            .map_or_else(|_| Value::BigInteger(text.into()), Value::Integer));
    }
    Ok(match json {
        JsonDoc::Object(object) => Value::Map(
            (object.iter())
                .map(|(key, value)| Ok((Key::from(key.as_str()), fake(value)?)))
                .collect::<Result<Map>>()?,
        ),
        JsonDoc::Array(items) => Value::List(items.iter().map(fake).collect::<Result<_>>()?),
        other => Value::from_serde(other),
    })
}

fn pairs(json: &JsonDoc) -> Vec<(String, String)> {
    (json.as_array().into_iter().flatten())
        .map(|pair| {
            (
                pair[0].as_str().unwrap_or("").into(),
                pair[1].as_str().unwrap_or("").into(),
            )
        })
        .collect()
}

fn fake_model(service: &str) -> Result<Arc<Model>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../tests/models/{service}/2020-01-01/service-2.json"
    ));
    Ok(model_from_json(&std::fs::read(path)?)?)
}

/// One `fakes.json` request: botocore's `serialize_to_request` exactly, or
/// its error's text.
fn fake_request(row: &JsonDoc) -> Result<()> {
    let model = fake_model(row["service"].as_str().ok_or("service")?)?;
    let operation = row["operation"].as_str().ok_or("operation")?;
    let request = serialize_case(protocol()?, &model, operation, &fake(&row["params"])?);
    let request = match (request, row["error"].as_str()) {
        (Err(error), Some(expected)) if error.message == expected => return Ok(()),
        (Ok(request), None) => request,
        (other, _) => return Err(format!("{other:?}").into()),
    };
    let expected = &row["serialized"];
    let headers: Vec<(String, String)> = (request.headers.iter())
        .map(|(n, v)| (n.to_owned(), v.to_owned()))
        .collect();
    let body = pytext::base64::decode(expected["body_base64"].as_str().ok_or("body")?)?;
    let checks = [
        (request.method.as_str() == expected["method"], "method"),
        (
            request.path == expected["url_path"] && request.query == expected["query"],
            "target",
        ),
        (headers == pairs(&expected["headers"]), "headers"),
        (request.body == body, "body"),
        (
            request.host_prefix.as_deref() == expected["host_prefix"].as_str(),
            "host prefix",
        ),
    ];
    match checks.iter().find(|(ok, _)| !ok) {
        None => Ok(()),
        Some((_, what)) => Err(format!(
            "{what}: {} {} {headers:?} {}\nexpected {expected}",
            request.method.as_str(),
            request.path,
            pytext::base64::encode(&request.body)
        )
        .into()),
    }
}

/// One `fakes.json` reply: botocore's `parse` exactly, and for an error the
/// service error `Endpoint` builds from it; or the error's text.
fn fake_response(row: &JsonDoc) -> Result<()> {
    let model = fake_model(row["service"].as_str().ok_or("service")?)?;
    let operation = row["operation"].as_str().ok_or("operation")?;
    let headers = pairs(&row["headers"]);
    let response = HttpResponse {
        status: u16::try_from(row["status"].as_u64().ok_or("status")?)?,
        headers: Headers::from_pairs(headers.iter().map(|(n, v)| (n.as_str(), v.as_str()))),
        body: pytext::base64::decode(row["body_base64"].as_str().ok_or("body")?)?,
    };
    let options = parse_options(&keep, &keep);
    let actual = if response.status < 300 {
        parse_case(protocol()?, &model, operation, &response, &options)
    } else {
        parse_error_case(protocol()?, &model, operation, &response, &options).map(|error| {
            let mut actual = Map::new();
            actual.insert("Error".into(), Value::Map(error.error));
            actual.insert(
                "RequestId".into(),
                error.request_id.map_or(Value::Null, Value::String),
            );
            actual.insert("members".into(), Value::Map(error.members));
            Value::Map(actual)
        })
    };
    let actual = match (actual, row["error"].as_str()) {
        (Err(error), Some(expected)) if error.message == expected => return Ok(()),
        (Ok(actual), None) => actual,
        (other, _) => return Err(format!("{other:?}").into()),
    };
    let Value::Map(mut expected) = fake(&row["parsed"])? else {
        return Err("parsed".into());
    };
    if response.status >= 300 {
        let request_id = match expected.remove("ResponseMetadata") {
            Some(Value::Map(metadata)) => metadata.get("RequestId").cloned(),
            _ => None,
        };
        let mut rebuilt = Map::new();
        // A 300 parses as an output: the service error's `Error` is empty.
        let error = expected.remove("Error").unwrap_or(Value::Map(Map::new()));
        rebuilt.insert("Error".into(), error);
        rebuilt.insert("RequestId".into(), request_id.unwrap_or(Value::Null));
        rebuilt.insert("members".into(), Value::Map(expected));
        expected = rebuilt;
    }
    if same(&actual, &Value::Map(expected.clone())) {
        Ok(())
    } else {
        Err(format!("parsed {actual:?}\nexpected {expected:?}").into())
    }
}

#[test]
fn botocore_s_rpc_v2_cbor_requests_and_replies_over_the_fakes_match() -> Result<()> {
    let fakes =
        read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/rpc_v2_cbor_suite/fakes.json"))?;
    let mut failed = Vec::new();
    let mut compared = 0;
    for (kind, check) in [
        ("requests", fake_request as fn(&JsonDoc) -> Result<()>),
        ("responses", fake_response),
    ] {
        for row in fakes[kind].as_array().ok_or(kind)? {
            compared += 1;
            if let Err(error) = check(row) {
                failed.push(format!("{kind} {}: {error}", row["id"]));
            }
        }
    }
    let report = format!(
        "fakes/{PROTOCOL}: compared {compared} failed {}\n",
        failed.len()
    );
    std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes())?;
    assert!(compared > 60, "only {compared} fakes rows");
    assert!(
        failed.is_empty(),
        "{} failed:\n{}",
        failed.len(),
        failed.join("\n")
    );
    Ok(())
}

/// The body comparison is no looser than botocore's where it matters: each
/// of these differs from `{"a": [1, "x"], "b": h'01'}` and must not compare
/// equal; what Python's `==` equates must.
#[test]
fn the_body_comparison_tells_apart_what_botocore_s_does() {
    let base = pytext::base64::encode(&from_hex("a261618201617861624101"));
    let body = |hex: &str| from_hex(hex);
    // Keys reordered, and `1` as `1.0`: equal under `==`.
    assert!(same_body(&body("a261624101616182016178"), &base));
    assert!(same_body(&body("a2616182f93c00617861624101"), &base));
    for different in [
        "a261618202617861624101",       // a scalar's value
        "a1616182016178",               // a missing key
        "a361618201617861624101616300", // an extra key
        "a261618261780161624101",       // a list reordered
        "a261618201617861626101",       // bytes against text
        "a2616182016178616241",         // a truncated body, which does not decode
        "",                             // no body against one
        "a26161820161786162c100",       // a tag where bytes were
    ] {
        assert!(!same_body(&body(different), &base), "{different}");
    }
    // `nan` below the top level equals nothing, as in Python.
    let nan = pytext::base64::encode(&body("a1616181f97e00"));
    assert!(!same_body(&body("a1616181f97e00"), &nan));
    // At the top it is the text `NaN` on both sides.
    let top = pytext::base64::encode(&body("a16161f97e00"));
    assert!(same_body(&body("a16161fb7ff8000000000000"), &top));
}

/// The decode vectors' check is not vacuous: a wrong value, kind or float
/// bit pattern fails it.
#[test]
fn the_decode_vector_check_tells_values_apart() -> Result<()> {
    let item = |hex: &str| decode(&from_hex(hex), &keep);
    let expect = |json: &str| serde_json::from_str::<JsonDoc>(json);
    assert!(expected_value(&item("17")?, &expect(r#"{"uint": 23}"#)?));
    assert!(!expected_value(&item("16")?, &expect(r#"{"uint": 23}"#)?));
    assert!(!expected_value(
        &item("6161")?,
        &expect(r#"{"bytestring": [97]}"#)?
    ));
    assert!(!expected_value(
        &item("fa3fd00001")?,
        &expect(r#"{"float32": 1070596096}"#)?
    ));
    assert!(!expected_value(
        &item("a163666f6f01")?,
        &expect(r#"{"map": {"foo": {"uint": 0}}}"#)?
    ));
    assert!(!expected_value(
        &item("a0")?,
        &expect(r#"{"map": {"foo": {"uint": 0}}}"#)?
    ));
    assert!(!expected_value(&item("00")?, &expect(r#"{"null": {}}"#)?));
    Ok(())
}
