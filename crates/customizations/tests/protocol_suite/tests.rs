//! botocore's protocol suite for the query, json and rest-json protocols,
//! replayed as `aws-cli/tests/unit/botocore/test_protocols.py` replays it:
//! the same per-case model, the same comparisons, and the same ignore list,
//! read from `protocol-tests-ignore-list.json` when the test runs.
use client::error::ParseError;
use client::http::{Headers, HttpResponse};
use client::testing::facade::{
    Model, ParseOptions, ProtocolHandle, input_members, model_from_json, parse_case,
    parse_error_case, protocol_named, serialize_case,
};
use model::value::{Map, Value};
use serde_json::Value as JsonDoc;
use std::path::PathBuf;
use std::sync::Arc;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const FILES: [&str; 4] = ["query", "json", "json_1_0", "json_1_0-query-compatible"];

fn protocols_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../aws-cli/tests/unit/botocore/protocols")
}

fn read(path: PathBuf) -> Result<JsonDoc> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

/// `_should_ignore_test`.
fn ignored(list: &JsonDoc, protocol: &str, kind: &str, suite: &str, id: Option<&str>) -> bool {
    let listed = |section: &JsonDoc| {
        let names = |key: &str| section[key].as_array().cloned().unwrap_or_default();
        names("suites")
            .iter()
            .any(|name| name.as_str() == Some(suite))
            || id.is_some_and(|id| names("cases").iter().any(|name| name.as_str() == Some(id)))
    };
    listed(&list["general"][kind]) || listed(&list["protocols"][protocol][kind])
}

/// Python's `==` over parsed values: dicts ignore order, numbers compare by
/// value across `int`, `float` and `bool`.
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

/// One case's model: the suite's description with the case as its only
/// operation, under `given.name` or `OperationName`.
fn case_model(suite: &JsonDoc, case: &JsonDoc, wrap_output: bool) -> Result<(Arc<Model>, String)> {
    let name = case["given"]["name"]
        .as_str()
        .unwrap_or("OperationName")
        .to_owned();
    let mut description = suite.clone();
    let object = description.as_object_mut().ok_or("suite")?;
    object.remove("cases");
    let mut given = case["given"].clone();
    if wrap_output {
        wrap_result(&mut given, suite, &name);
    }
    let mut operations = serde_json::Map::new();
    operations.insert(name.clone(), given);
    object.insert("operations".into(), JsonDoc::Object(operations));
    Ok((model_from_json(&serde_json::to_vec(&description)?)?, name))
}

/// The query output case's `resultWrapper`, set when the output shape has
/// members (`test_output_compliance`).
fn wrap_result(given: &mut JsonDoc, suite: &JsonDoc, name: &str) {
    let Some(output) = given.get_mut("output").and_then(JsonDoc::as_object_mut) else {
        return;
    };
    let shape = output
        .get("shape")
        .and_then(JsonDoc::as_str)
        .unwrap_or_default();
    let members = &suite["shapes"][shape]["members"];
    if members
        .as_object()
        .is_some_and(|members| !members.is_empty())
    {
        output.insert(
            "resultWrapper".into(),
            JsonDoc::String(format!("{name}Result")),
        );
    }
}

/// `_convert_strings_to_special_float`, on the top-level parameters.
fn special_floats_in(params: Value) -> Value {
    let Value::Map(map) = params else {
        return params;
    };
    let converted = map.into_iter().map(|(key, value)| {
        let value = match &value {
            Value::String(text) if text == "Infinity" => Value::Float(f64::INFINITY),
            Value::String(text) if text == "-Infinity" => Value::Float(f64::NEG_INFINITY),
            Value::String(text) if text == "NaN" => Value::Float(f64::NAN),
            _ => value,
        };
        (key, value)
    });
    Value::Map(converted.collect())
}

/// `_convert_special_floats_to_string`, on the top level.
fn special_floats_out(parsed: Map) -> Map {
    parsed
        .into_iter()
        .map(|(key, value)| {
            let value = match value {
                Value::Float(n) if n == f64::INFINITY => Value::String("Infinity".into()),
                Value::Float(n) if n == f64::NEG_INFINITY => Value::String("-Infinity".into()),
                Value::Float(n) if n.is_nan() => Value::String("NaN".into()),
                other => other,
            };
            (key, value)
        })
        .collect()
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

fn protocol(name: &str) -> Result<ProtocolHandle> {
    let name = match name {
        "query" | "rest-json" => name,
        _ => "json",
    };
    Ok(protocol_named(name).ok_or("protocol")?)
}

/// The protocol a suite file's cases speak.
fn protocol_of(file: &str) -> &str {
    match file {
        "query" | "rest-json" => file,
        _ => "json",
    }
}

/// `test_input_compliance` for one case.
fn input_case(protocol_name: &str, suite: &JsonDoc, case: &JsonDoc) -> Result<()> {
    let (model, name) = case_model(suite, case, false)?;
    let params = special_floats_in(Value::from_serde(&case["params"]));
    let request = serialize_case(protocol(protocol_name)?, &model, &name, &params)?;
    let expected = &case["serialized"];
    if let Some(body) = expected["body"].as_str()
        && !same_body(protocol_name, &request.body, body.as_bytes())?
    {
        return Err(format!(
            "body {:?} != {body:?}",
            String::from_utf8_lossy(&request.body)
        )
        .into());
    }
    let lists = list_headers(&model, &name);
    check_headers(protocol_name, &lists, &request.headers, expected)?;
    let mut path = request.path.clone();
    if !request.query.is_empty() {
        path = format!("{path}?{}", request.query);
    }
    let uri = expected["uri"].as_str().unwrap_or("");
    if path != uri {
        return Err(format!("uri {path} != {uri}").into());
    }
    if let Some(method) = expected["method"]
        .as_str()
        .filter(|m| *m != request.method.as_str())
    {
        return Err(format!("method {} != {method}", request.method.as_str()).into());
    }
    check_host(suite, &request.host_prefix, expected)
}

/// `_assert_request_body`: query bodies as bytes; json and rest-json
/// bodies as parsed JSON (`_assert_json_bodies`), a rest-json body that is
/// not JSON, on either side, as bytes.
fn same_body(protocol_name: &str, actual: &[u8], expected: &[u8]) -> Result<bool> {
    if protocol_name == "query" {
        return Ok(actual == expected);
    }
    match (Value::from_json(actual), Value::from_json(expected)) {
        (Ok(actual), Ok(expected)) => Ok(python_eq(&actual, &expected)),
        _ if protocol_name == "rest-json" => Ok(actual == expected),
        (Err(error), _) | (_, Err(error)) => Err(error.into()),
    }
}

/// The input's `header` list members `_clean_list_header_values` reads:
/// each header's name, and whether its items are timestamps.
fn list_headers(model: &Model, operation: &str) -> Vec<(String, bool)> {
    input_members(model, operation)
        .into_iter()
        .filter_map(|member| {
            if member.type_name != "list" {
                return None;
            }
            let name = member.location_name?;
            (member.location.as_deref() == Some("header")).then(|| {
                let timestamps = member.item_type_name.as_deref() == Some("timestamp");
                (name, timestamps)
            })
        })
        .collect()
}

/// `_assert_expected_headers_in_request`, after `_clean_list_header_values`
/// takes the spaces after the commas of list headers out of the expected
/// values, and out of the actual values of timestamp lists.
fn check_headers(
    protocol_name: &str,
    lists: &[(String, bool)],
    actual: &Headers,
    expected: &JsonDoc,
) -> Result<()> {
    let list = |name: &str| {
        lists
            .iter()
            .find(|(list, _)| list.eq_ignore_ascii_case(name))
    };
    let mut wanted: Vec<(String, String)> = expected["headers"]
        .as_object()
        .map(|headers| {
            headers
                .iter()
                .map(|(k, v)| {
                    let value = v.as_str().unwrap_or_default();
                    let value = match list(k) {
                        Some(_) => value.replace(", ", ","),
                        None => value.to_owned(),
                    };
                    (k.clone(), value)
                })
                .collect()
        })
        .unwrap_or_default();
    let content_type = wanted
        .iter()
        .position(|(k, _)| k.eq_ignore_ascii_case("Content-Type"));
    if protocol_name == "query" {
        let current = content_type
            .map(|i| wanted[i].1.clone())
            .unwrap_or_default();
        if !current.contains("charset=utf-8") {
            let value = format!("{current}; charset=utf-8");
            match content_type {
                Some(i) => wanted[i].1 = value,
                None => wanted.push(("Content-Type".into(), value)),
            }
        }
    } else if protocol_name == "rest-json"
        && let Some(i) = content_type
        && !wanted[i].1.is_empty()
        && wanted[i].1 != "application/json"
    {
        // Streaming payloads get no `Content-Type` from botocore.
        wanted.remove(i);
    }
    for (name, value) in wanted {
        let got = actual.get(&name).map(|got| match list(&name) {
            Some((_, true)) => got.replace(", ", ","),
            _ => got.to_owned(),
        });
        if got.as_deref() != Some(value.as_str()) {
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

/// `_assert_endpoints_equal`: the host the prefix and `clientEndpoint` give.
fn check_host(suite: &JsonDoc, prefix: &Option<String>, expected: &JsonDoc) -> Result<()> {
    let Some(host) = expected["host"].as_str() else {
        return Ok(());
    };
    let endpoint = suite["clientEndpoint"].as_str().ok_or("clientEndpoint")?;
    let netloc = endpoint
        .split_once("://")
        .map_or(endpoint, |(_, rest)| rest);
    let netloc = netloc.split('/').next().unwrap_or(netloc);
    let actual = format!("{}{netloc}", prefix.as_deref().unwrap_or(""));
    if actual == host {
        Ok(())
    } else {
        Err(format!("host {actual} != {host}").into())
    }
}

/// `_compliance_timestamp_parser`: seconds since the epoch, as a float.
fn compliance_timestamp(value: &Value) -> std::result::Result<Value, ParseError> {
    match (ParseOptions::BOTOCORE.timestamp)(value)? {
        Value::Timestamp(at) => Ok(Value::Float(at.unix_micros() as f64 / 1e6)),
        other => Ok(other),
    }
}

/// `test_output_compliance` for one case.
fn output_case(protocol_name: &str, suite: &JsonDoc, case: &JsonDoc) -> Result<()> {
    let is_error = case.get("error").is_some();
    let (model, name) = case_model(suite, case, protocol_name == "query" && !is_error)?;
    let raw = &case["response"];
    let body = match raw["body"].as_str() {
        Some(body) => body.as_bytes().to_vec(),
        None if protocol_name == "query" => b"<xml/>".to_vec(),
        None => Vec::new(),
    };
    let headers = raw["headers"].as_object().map(|headers| {
        Headers::from_pairs(
            headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str().unwrap_or_default())),
        )
    });
    let status = raw["status_code"]
        .as_u64()
        .and_then(|s| u16::try_from(s).ok())
        .unwrap_or(200);
    let response = HttpResponse {
        status,
        headers: headers.unwrap_or_default(),
        body,
    };
    let options = ParseOptions {
        timestamp: &compliance_timestamp,
        blob: ParseOptions::BOTOCORE.blob,
    };
    let (parsed, expected) = if is_error {
        let error = parse_error_case(protocol(protocol_name)?, &model, &name, &response, &options)?;
        let mut parsed = Map::new();
        let kept: Map = error
            .error
            .iter()
            .filter(|(k, _)| matches!(k.as_str(), Some("Code" | "Message")))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        parsed.insert("Error".into(), Value::Map(kept));
        for (key, value) in error.members {
            parsed.insert_key(key, value);
        }
        (parsed, expected_error(case))
    } else {
        let Value::Map(mut parsed) =
            parse_case(protocol(protocol_name)?, &model, &name, &response, &options)?
        else {
            return Err("not a structure".into());
        };
        parsed.remove("ResponseMetadata");
        (parsed, Value::from_serde(&case["result"]))
    };
    let parsed = Value::Map(special_floats_out(
        match bytes_to_text(Value::Map(parsed)) {
            Value::Map(map) => map,
            _ => Map::new(),
        },
    ));
    if python_eq(&parsed, &expected) {
        Ok(())
    } else {
        Err(format!("parsed {parsed:?}\nexpected {expected:?}").into())
    }
}

/// `{'Error': {'Code': errorCode, 'Message': errorMessage}}` updated with
/// the case's `error`.
fn expected_error(case: &JsonDoc) -> Value {
    let field = |key: &str| Value::String(case[key].as_str().unwrap_or("").to_owned());
    let mut error = Map::new();
    error.insert("Code".into(), field("errorCode"));
    error.insert("Message".into(), field("errorMessage"));
    let mut expected = Map::new();
    expected.insert("Error".into(), Value::Map(error));
    if let Value::Map(members) = Value::from_serde(&case["error"]) {
        for (key, value) in members {
            expected.insert_key(key, value);
        }
    }
    Value::Map(expected)
}

#[derive(Default)]
struct Tally {
    passed: usize,
    failed: Vec<String>,
    ignored: usize,
    ignored_passing: usize,
}

fn run_file(kind: &str, file: &str, list: &JsonDoc) -> Result<Tally> {
    let suites = read(protocols_dir().join(kind).join(format!("{file}.json")))?;
    let protocol_name = protocol_of(file);
    let mut tally = Tally::default();
    for (suite_index, suite) in suites.as_array().ok_or("suites")?.iter().enumerate() {
        let description = suite["description"].as_str().unwrap_or_default();
        for (case_index, case) in suite["cases"].as_array().into_iter().flatten().enumerate() {
            let key = if kind == "input" {
                "params"
            } else {
                "response"
            };
            if case.get(key).is_none() {
                continue;
            }
            let outcome = if kind == "input" {
                input_case(protocol_name, suite, case)
            } else {
                output_case(protocol_name, suite, case)
            };
            if ignored(list, file, kind, description, case["id"].as_str()) {
                tally.ignored += 1;
                tally.ignored_passing += usize::from(outcome.is_ok());
            } else if let Err(error) = outcome {
                let id = case["id"].as_str().unwrap_or("");
                tally.failed.push(format!(
                    "{kind}/{file} {description} ({suite_index}:{case_index}) {id}: {error}"
                ));
            } else {
                tally.passed += 1;
            }
        }
    }
    Ok(tally)
}

/// Every case of `files` for each of `kinds`, a report of the counts on
/// stderr, and a failure naming each case that failed.
fn run_suites(kinds: &[&str], files: &[&str]) -> Result<()> {
    let list = read(protocols_dir().join("protocol-tests-ignore-list.json"))?;
    let mut report =
        String::from("file                                   passed failed ignored(passing)\n");
    let mut failures = Vec::new();
    let mut passed = 0;
    for kind in kinds {
        for file in files {
            let tally = run_file(kind, file, &list)?;
            report.push_str(&format!(
                "{:<38} {:>6} {:>6} {:>7}({})\n",
                format!("{kind}/{file}"),
                tally.passed,
                tally.failed.len(),
                tally.ignored,
                tally.ignored_passing
            ));
            passed += tally.passed;
            failures.extend(tally.failed);
        }
    }
    report.push_str(&format!("total passed: {passed}\n"));
    std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes())?;
    assert!(
        failures.is_empty(),
        "{} failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    Ok(())
}

#[test]
fn botocore_s_query_and_json_protocol_suites_pass() -> Result<()> {
    run_suites(&["input", "output"], &FILES)
}

#[test]
fn botocore_s_rest_json_input_suite_passes() -> Result<()> {
    run_suites(&["input"], &["rest-json"])
}

#[test]
fn botocore_s_rest_json_output_suite_passes() -> Result<()> {
    run_suites(&["output"], &["rest-json"])
}
