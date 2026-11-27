//! botocore's ec2 protocol suite, replayed as
//! `aws-cli/tests/unit/botocore/test_protocols.py` replays it with
//! `EC2Serializer` and `EC2QueryParser`: the same per-case model, the same
//! comparisons, and the same ignore list, read from
//! `protocol-tests-ignore-list.json` when the test runs. The protocol comes
//! from the façade's registry of built protocols, which holds the admitted ones.
use client::error::ParseError;
use client::http::{Headers, HttpResponse};
use client::testing::facade::{
    Model, ProtocolHandle, botocore_blob, botocore_timestamp, built_protocol_named,
    model_from_json, parse_case, parse_error_case, parse_options, protocol_named, serialize_case,
};
use model::value::{Map, Value};
use serde_json::Value as JsonDoc;
use std::path::PathBuf;
use std::sync::Arc;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const FILE: &str = "ec2";

fn protocols_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../aws-cli/tests/unit/botocore/protocols")
}

fn read(path: PathBuf) -> Result<JsonDoc> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

fn ec2() -> Result<ProtocolHandle> {
    Ok(built_protocol_named(FILE).ok_or("ec2 is not built")?)
}

/// `_should_ignore_test`.
fn ignored(list: &JsonDoc, kind: &str, suite: &str, id: Option<&str>) -> bool {
    let listed = |section: &JsonDoc| {
        let names = |key: &str| section[key].as_array().cloned().unwrap_or_default();
        names("suites")
            .iter()
            .any(|name| name.as_str() == Some(suite))
            || id.is_some_and(|id| names("cases").iter().any(|name| name.as_str() == Some(id)))
    };
    listed(&list["general"][kind]) || listed(&list["protocols"][FILE][kind])
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
/// operation, under `given.name` or `OperationName`. The ec2 output gets no
/// `resultWrapper` (only `query` does).
fn case_model(suite: &JsonDoc, case: &JsonDoc) -> Result<(Arc<Model>, String)> {
    let name = case["given"]["name"]
        .as_str()
        .unwrap_or("OperationName")
        .to_owned();
    let mut description = suite.clone();
    let object = description.as_object_mut().ok_or("suite")?;
    object.remove("cases");
    let mut operations = serde_json::Map::new();
    operations.insert(name.clone(), case["given"].clone());
    object.insert("operations".into(), JsonDoc::Object(operations));
    if object["metadata"]["protocol"] != FILE {
        return Err("not an ec2 suite".into());
    }
    Ok((model_from_json(&serde_json::to_vec(&description)?)?, name))
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

/// `test_input_compliance` for one case: the body as bytes, the expected
/// headers (`Content-Type` with `; charset=utf-8` added) and no forbidden
/// one, the URI, the method, and the host.
fn input_case(protocol: ProtocolHandle, suite: &JsonDoc, case: &JsonDoc) -> Result<()> {
    let (model, name) = case_model(suite, case)?;
    let params = special_floats_in(Value::from_serde(&case["params"]));
    let request = serialize_case(protocol, &model, &name, &params)?;
    let expected = &case["serialized"];
    if let Some(body) = expected["body"].as_str()
        && request.body != body.as_bytes()
    {
        return Err(format!(
            "body {:?} != {body:?}",
            String::from_utf8_lossy(&request.body)
        )
        .into());
    }
    check_headers(&request.headers, expected)?;
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

/// `_assert_expected_headers_in_request` for `ec2`. The protocol binds no
/// header, so `_clean_list_header_values` changes nothing.
fn check_headers(actual: &Headers, expected: &JsonDoc) -> Result<()> {
    let mut wanted: Vec<(String, String)> = expected["headers"]
        .as_object()
        .map(|headers| {
            let pairs = headers.iter();
            pairs
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
                .collect()
        })
        .unwrap_or_default();
    let content_type = wanted
        .iter()
        .position(|(k, _)| k.eq_ignore_ascii_case("Content-Type"));
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
    for (name, value) in wanted {
        let got = actual.get(&name);
        if got != Some(value.as_str()) {
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
    match botocore_timestamp(value)? {
        Value::Timestamp(at) => Ok(Value::Float(at.unix_micros() as f64 / 1e6)),
        other => Ok(other),
    }
}

/// `test_output_compliance` for one case. An error case compares the whole
/// parsed dict, as Python's `assert_equal` does: every field of `Error`
/// and every other key the error parse leaves at the top.
fn output_case(protocol: ProtocolHandle, suite: &JsonDoc, case: &JsonDoc) -> Result<()> {
    let is_error = case.get("error").is_some();
    let (model, name) = case_model(suite, case)?;
    let raw = &case["response"];
    // `b''` for a case without a body: the `<xml/>` default is query's.
    let body = raw["body"].as_str().unwrap_or("").as_bytes().to_vec();
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
    let options = parse_options(&compliance_timestamp, &botocore_blob);
    let (parsed, expected) = if is_error {
        let error = parse_error_case(protocol, &model, &name, &response, &options)?;
        let mut parsed = Map::new();
        parsed.insert("Error".into(), Value::Map(error.error));
        for (key, value) in error.members {
            parsed.insert_key(key, value);
        }
        (parsed, expected_error(case))
    } else {
        let Value::Map(mut parsed) = parse_case(protocol, &model, &name, &response, &options)?
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

#[derive(Debug, Default)]
struct Tally {
    passed: usize,
    failed: Vec<String>,
    ignored: usize,
    ignored_passing: usize,
}

/// Every case of `kind` under `protocol`, the ignored ones run but not
/// counted, and a report of the counts on stderr.
fn run(protocol: ProtocolHandle, kind: &str) -> Result<Tally> {
    let list = read(protocols_dir().join("protocol-tests-ignore-list.json"))?;
    let suites = read(protocols_dir().join(kind).join(format!("{FILE}.json")))?;
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
                input_case(protocol, suite, case)
            } else {
                output_case(protocol, suite, case)
            };
            if ignored(&list, kind, description, case["id"].as_str()) {
                tally.ignored += 1;
                tally.ignored_passing += usize::from(outcome.is_ok());
            } else if let Err(error) = outcome {
                let id = case["id"].as_str().unwrap_or("");
                tally.failed.push(format!(
                    "{kind}/{FILE} {description} ({suite_index}:{case_index}) {id}: {error}"
                ));
            } else {
                tally.passed += 1;
            }
        }
    }
    let report = format!(
        "{kind}/{FILE}: {} passed, {} failed, {} ignored ({} of them passing)\n",
        tally.passed,
        tally.failed.len(),
        tally.ignored,
        tally.ignored_passing
    );
    std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes())?;
    Ok(tally)
}

/// Every case of a kind passes but the ignored ones, and the suite is not
/// empty: a file that stopped loading cases would pass vacuously.
fn assert_suite(kind: &str, at_least: usize, ignored: usize) -> Result<()> {
    let tally = run(ec2()?, kind)?;
    assert!(
        tally.failed.is_empty(),
        "{} failed:\n{}",
        tally.failed.len(),
        tally.failed.join("\n")
    );
    assert!(
        tally.passed >= at_least,
        "only {} {kind} cases ran",
        tally.passed
    );
    // As many ignored as the list names for ec2's cases: an ignore rule that
    // matched more would hide failures.
    assert_eq!(tally.ignored, ignored, "ignored {kind} cases");
    Ok(())
}

#[test]
fn botocore_s_ec2_input_suite_passes() -> Result<()> {
    assert_suite("input", 20, 8)
}

#[test]
fn botocore_s_ec2_output_suite_passes() -> Result<()> {
    assert_suite("output", 25, 2)
}

/// Admitted: the façade's admitted registry holds `ec2`, the protocol
/// admission and clients reach.
#[test]
fn ec2_is_reachable_through_the_admitted_registry() {
    assert!(protocol_named(FILE).is_some());
    assert!(built_protocol_named(FILE).is_some());
    assert!(built_protocol_named("query").is_some());
}

/// The suites tell the protocols apart: awsQuery, whose serializer and
/// parser ec2's only adjust, fails cases of each.
#[test]
fn the_suites_fail_the_query_protocol() -> Result<()> {
    let query = protocol_named("query").ok_or("query")?;
    // Output: the error case, by its `RequestID` left at the top, which a
    // comparison of `Code` and `Message` alone would miss.
    for (kind, at_least) in [("input", 3), ("output", 1)] {
        let tally = run(query, kind)?;
        assert!(tally.failed.len() >= at_least, "{kind}: {:?}", tally.failed);
    }
    Ok(())
}

/// A missing suite file fails rather than passing empty.
#[test]
fn a_missing_suite_fails() -> Result<()> {
    assert!(run(ec2()?, "absent").is_err());
    Ok(())
}

/// The header check fails a wrong or missing `Content-Type`, a missing
/// expected header and a forbidden one.
#[test]
fn the_header_check_is_strict() {
    let sent = |pairs: &[(&str, &str)]| Headers::from_pairs(pairs.iter().copied());
    let form = "application/x-www-form-urlencoded; charset=utf-8";
    let expected = serde_json::json!({
        "headers": {"Content-Type": "application/x-www-form-urlencoded", "X-A": "1"},
        "forbidHeaders": ["X-B"],
    });
    assert!(check_headers(&sent(&[("Content-Type", form), ("X-A", "1")]), &expected).is_ok());
    let wrong = [
        ("Content-Type", "application/x-www-form-urlencoded"),
        ("X-A", "1"),
    ];
    assert!(check_headers(&sent(&wrong), &expected).is_err());
    assert!(check_headers(&sent(&[("X-A", "1")]), &expected).is_err());
    assert!(check_headers(&sent(&[("Content-Type", form)]), &expected).is_err());
    let forbidden = [("Content-Type", form), ("X-A", "1"), ("X-B", "2")];
    assert!(check_headers(&sent(&forbidden), &expected).is_err());
}

/// Python's `==` sees an extra key, a missing one, a nested difference and
/// a different list order, and only numbers cross types.
#[test]
fn python_equality_is_strict_where_python_is() -> Result<()> {
    let value = |text: &str| Value::from_json(text.as_bytes());
    let base = value(r#"{"Error": {"Code": "C", "Message": "m"}, "A": [1, 2]}"#)?;
    for other in [
        r#"{"Error": {"Code": "C", "Message": "m", "Type": "Sender"}, "A": [1, 2]}"#,
        r#"{"Error": {"Code": "C"}, "A": [1, 2]}"#,
        r#"{"Error": {"Code": "C", "Message": "m"}, "A": [2, 1]}"#,
        r#"{"Error": {"Code": "C", "Message": "m"}, "A": [1, 2], "B": null}"#,
        r#"{"Error": {"Code": "C", "Message": "m"}, "A": ["1", 2]}"#,
    ] {
        assert!(!python_eq(&base, &value(other)?), "{other}");
    }
    let same = value(r#"{"A": [1.0, true], "Error": {"Message": "m", "Code": "C"}}"#)?;
    let base = value(r#"{"Error": {"Code": "C", "Message": "m"}, "A": [1, 1]}"#)?;
    assert!(python_eq(&base, &same));
    Ok(())
}

// The lint keeps `#[test]`s in files named tests.rs; this one is the fake rows'.
#[path = "fakes/tests.rs"]
mod fakes;
