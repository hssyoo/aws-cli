//! The rest-xml suite's replay and the fakes' comparison; see the crate
//! file's docs.
use client::error::ParseError;
use client::http::{Headers, HttpResponse};
use client::testing::facade::{
    Model, ProtocolHandle, botocore_blob, botocore_timestamp, built_protocol_named, input_members,
    model_from_json, parse_case, parse_error_case, parse_options, serialize_case,
};
use client::xml::Element;
use model::value::{Key, Map, Value};
use serde_json::Value as JsonDoc;
use std::path::PathBuf;
use std::sync::Arc;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const FILE: &str = "rest-xml";

fn protocols_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../aws-cli/tests/unit/botocore/protocols")
}

fn read(path: PathBuf) -> Result<JsonDoc> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

fn protocol() -> Result<ProtocolHandle> {
    Ok(built_protocol_named(FILE).ok_or("rest-xml is not built")?)
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
/// operation, under `given.name` or `OperationName`.
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

/// `ElementTree.canonicalize(body, strip_text=True)`, as a comparable
/// text; `None` when the body is not XML (`ET.ParseError`).
fn canonical(body: &[u8]) -> Option<String> {
    let root = client::xml::parse(body).ok()?;
    let mut out = String::new();
    write_canonical(&root, &mut out);
    let mut declarations = namespace_declarations(body);
    declarations.sort();
    out.push_str(&format!("\nxmlns {declarations:?}"));
    Some(out)
}

fn write_canonical(element: &Element, out: &mut String) {
    let mut attributes = element.attributes.clone();
    attributes.sort();
    out.push_str(&format!("<{} {attributes:?}>", element.tag));
    out.push_str(&format!(
        "{:?}",
        element.text.as_deref().unwrap_or("").trim()
    ));
    for child in &element.children {
        write_canonical(child, out);
    }
    out.push_str("</>");
}

/// Every `xmlns` or `xmlns:prefix` declaration in the document, as its
/// name and quoted value, which the parsed tree does not keep.
fn namespace_declarations(body: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(body);
    let mut found = Vec::new();
    let mut rest = &*text;
    while let Some(at) = rest.find("xmlns") {
        let after = &rest[at..];
        let name_end = after.find(['=', ' ', '>', '/']).unwrap_or(after.len());
        let name = after[..name_end].to_owned();
        let value = after[name_end..]
            .strip_prefix('=')
            .and_then(|tail| {
                let quote = tail.chars().next()?;
                let tail = &tail[1..];
                tail.find(quote).map(|end| tail[..end].to_owned())
            })
            .unwrap_or_default();
        found.push((name, value));
        rest = &after[5..];
    }
    found
}

/// `_assert_xml_bodies`.
fn same_body(actual: &[u8], expected: &[u8]) -> bool {
    match (canonical(actual), canonical(expected)) {
        (Some(actual), Some(expected)) => actual == expected,
        _ => actual == expected,
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

/// `_assert_expected_headers_in_request` for rest-xml: the expected
/// `Content-Type` dropped, list headers cleaned of the spaces after their
/// commas (the actual ones only for timestamp lists), every other expected
/// header present with its value, and no forbidden one.
fn check_headers(lists: &[(String, bool)], actual: &Headers, expected: &JsonDoc) -> Result<()> {
    let list = |name: &str| {
        lists
            .iter()
            .find(|(list, _)| list.eq_ignore_ascii_case(name))
    };
    let wanted = expected["headers"].as_object().into_iter().flatten();
    for (name, value) in wanted {
        if name.eq_ignore_ascii_case("Content-Type") {
            continue;
        }
        let value = value.as_str().unwrap_or_default();
        let value = match list(name) {
            Some(_) => value.replace(", ", ","),
            None => value.to_owned(),
        };
        let got = actual.get(name).map(|got| match list(name) {
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
fn check_host(suite: &JsonDoc, prefix: Option<&str>, expected: &JsonDoc) -> Result<()> {
    let Some(host) = expected["host"].as_str() else {
        return Ok(());
    };
    let endpoint = suite["clientEndpoint"].as_str().ok_or("clientEndpoint")?;
    let netloc = endpoint
        .split_once("://")
        .map_or(endpoint, |(_, rest)| rest);
    let netloc = netloc.split('/').next().unwrap_or(netloc);
    let actual = format!("{}{netloc}", prefix.unwrap_or(""));
    if actual == host {
        Ok(())
    } else {
        Err(format!("host {actual} != {host}").into())
    }
}

/// `test_input_compliance` for one case.
fn input_case(suite: &JsonDoc, case: &JsonDoc) -> Result<()> {
    let (model, name) = case_model(suite, case)?;
    let params = special_floats_in(Value::from_serde(&case["params"]));
    let request = serialize_case(protocol()?, &model, &name, &params)?;
    let expected = &case["serialized"];
    if let Some(body) = expected["body"].as_str()
        && !same_body(&request.body, body.as_bytes())
    {
        let actual = String::from_utf8_lossy(&request.body);
        return Err(format!("body {actual:?} != {body:?}").into());
    }
    check_headers(&list_headers(&model, &name), &request.headers, expected)?;
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
    check_host(suite, request.host_prefix.as_deref(), expected)
}

/// `_compliance_timestamp_parser`: seconds since the epoch, as a float.
fn compliance_timestamp(value: &Value) -> std::result::Result<Value, ParseError> {
    match botocore_timestamp(value)? {
        Value::Timestamp(at) => Ok(Value::Float(at.unix_micros() as f64 / 1e6)),
        other => Ok(other),
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

/// The response a case describes: no body is `b''`.
fn case_response(raw: &JsonDoc) -> HttpResponse {
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
    HttpResponse {
        status,
        headers: headers.unwrap_or_default(),
        body: raw["body"].as_str().unwrap_or("").as_bytes().to_vec(),
    }
}

/// `test_output_compliance` for one case, after `_fixup_parsed_result`.
fn output_case(suite: &JsonDoc, case: &JsonDoc) -> Result<()> {
    let (model, name) = case_model(suite, case)?;
    let response = case_response(&case["response"]);
    let options = parse_options(&compliance_timestamp, &botocore_blob);
    let (parsed, expected) = if case.get("error").is_some() {
        let error = parse_error_case(protocol()?, &model, &name, &response, &options)?;
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
        let Value::Map(mut parsed) = parse_case(protocol()?, &model, &name, &response, &options)?
        else {
            return Err("not a structure".into());
        };
        parsed.remove("ResponseMetadata");
        (parsed, Value::from_serde(&case["result"]))
    };
    let parsed = match bytes_to_text(Value::Map(parsed)) {
        Value::Map(map) => Value::Map(special_floats_out(map)),
        other => other,
    };
    if python_eq(&parsed, &expected) {
        Ok(())
    } else {
        Err(format!("parsed {parsed:?}\nexpected {expected:?}").into())
    }
}

/// Every `kind` case of the rest-xml file: a report of the counts on
/// stderr, and a failure naming each case that failed.
fn run_suite(kind: &str) -> Result<()> {
    let list = read(protocols_dir().join("protocol-tests-ignore-list.json"))?;
    let suites = read(protocols_dir().join(kind).join(format!("{FILE}.json")))?;
    let (mut passed, mut ignored_cases, mut ignored_passing) = (0, 0, 0);
    let mut failed = Vec::new();
    let key = if kind == "input" {
        "params"
    } else {
        "response"
    };
    for (suite_index, suite) in suites.as_array().ok_or("suites")?.iter().enumerate() {
        let description = suite["description"].as_str().unwrap_or_default();
        for (case_index, case) in suite["cases"].as_array().into_iter().flatten().enumerate() {
            if case.get(key).is_none() {
                continue;
            }
            let outcome = if kind == "input" {
                input_case(suite, case)
            } else {
                output_case(suite, case)
            };
            let id = case["id"].as_str();
            if ignored(&list, kind, description, id) {
                ignored_cases += 1;
                ignored_passing += usize::from(outcome.is_ok());
            } else if let Err(error) = outcome {
                let id = id.unwrap_or("");
                failed.push(format!(
                    "{kind}/{FILE} {description} ({suite_index}:{case_index}) {id}: {error}"
                ));
            } else {
                passed += 1;
            }
        }
    }
    let report = format!(
        "{kind}/{FILE}: passed {passed} failed {} ignored {ignored_cases} ({ignored_passing} \
         passing)\n",
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
fn botocore_s_rest_xml_input_suite_passes() -> Result<()> {
    run_suite("input")
}

#[test]
fn botocore_s_rest_xml_output_suite_passes() -> Result<()> {
    run_suite("output")
}

fn fakes_model() -> Result<Arc<Model>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/models/zrestxml/2020-01-01/service-2.json");
    Ok(model_from_json(&std::fs::read(path)?)?)
}

/// An input as `fakes.json` spells it: `{"$blob": base64}` is bytes and
/// `{"$float": text}` a float JSON cannot hold.
fn fake_input(json: &JsonDoc) -> Result<Value> {
    Ok(match json {
        JsonDoc::Object(object) if object.len() == 1 && object.contains_key("$blob") => {
            let text = object["$blob"].as_str().ok_or("$blob")?;
            Value::Blob(pytext::base64::decode(text)?)
        }
        JsonDoc::Object(object) if object.len() == 1 && object.contains_key("$float") => {
            Value::Float(object["$float"].as_str().ok_or("$float")?.parse()?)
        }
        JsonDoc::Object(object) => Value::Map(
            object
                .iter()
                .map(|(key, value)| Ok((Key::from(key.as_str()), fake_input(value)?)))
                .collect::<Result<Map>>()?,
        ),
        JsonDoc::Array(items) => Value::List(items.iter().map(fake_input).collect::<Result<_>>()?),
        other => Value::from_serde(other),
    })
}

/// One `fakes.json` request: botocore's `serialize_to_request` exactly.
fn fake_request(model: &Model, row: &JsonDoc) -> Result<()> {
    let operation = row["operation"].as_str().ok_or("operation")?;
    let request = serialize_case(protocol()?, model, operation, &fake_input(&row["params"])?)?;
    let expected = &row["serialized"];
    let url_path = expected["url_path"].as_str().ok_or("url_path")?;
    let query = expected["query"].as_str().ok_or("query")?;
    let target = match (query.is_empty(), url_path.contains('?')) {
        (true, _) => url_path.to_owned(),
        (false, true) => format!("{url_path}&{query}"),
        (false, false) => format!("{url_path}?{query}"),
    };
    let actual_target = match request.query.as_str() {
        "" => request.path.clone(),
        query => format!("{}?{query}", request.path),
    };
    let headers: Vec<(String, String)> = (request.headers.iter())
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    let expected_headers: Vec<(String, String)> = expected["headers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|pair| {
            (
                pair[0].as_str().unwrap_or("").into(),
                pair[1].as_str().unwrap_or("").into(),
            )
        })
        .collect();
    let body = pytext::base64::decode(expected["body_base64"].as_str().ok_or("body")?)?;
    let checks = [
        (request.method.as_str() == expected["method"], "method"),
        (actual_target == target, "target"),
        (headers == expected_headers, "headers"),
        (request.body == body, "body"),
    ];
    match checks.iter().find(|(ok, _)| !ok) {
        None => Ok(()),
        Some((_, what)) => Err(format!(
            "{what}: {} {actual_target} {headers:?} {:?}\nexpected {expected}",
            request.method.as_str(),
            String::from_utf8_lossy(&request.body)
        )
        .into()),
    }
}

/// One `fakes.json` reply: botocore's `parse` exactly, and for an error
/// the service error `Endpoint` builds from it.
fn fake_response(model: &Model, row: &JsonDoc) -> Result<()> {
    let operation = row["operation"].as_str().ok_or("operation")?;
    let pairs = row["headers"].as_array().into_iter().flatten();
    let response = HttpResponse {
        status: u16::try_from(row["status"].as_u64().ok_or("status")?)?,
        headers: Headers::from_pairs(pairs.map(|pair| {
            (
                pair[0].as_str().unwrap_or(""),
                pair[1].as_str().unwrap_or(""),
            )
        })),
        body: row["body"].as_str().ok_or("body")?.as_bytes().to_vec(),
    };
    let keep = |value: &Value| Ok(value.clone());
    let options = parse_options(&keep, &keep);
    let Value::Map(mut expected) = Value::from_serde(&row["parsed"]) else {
        return Err("parsed".into());
    };
    let actual = if response.status < 301 {
        parse_case(protocol()?, model, operation, &response, &options)?
    } else {
        let error = parse_error_case(protocol()?, model, operation, &response, &options)?;
        let metadata = expected.remove("ResponseMetadata");
        let request_id = match &metadata {
            Some(Value::Map(metadata)) => metadata.get("RequestId").cloned(),
            _ => None,
        };
        let mut rebuilt = Map::new();
        if let Some(error) = expected.remove("Error") {
            rebuilt.insert("Error".into(), error);
        }
        rebuilt.insert("RequestId".into(), request_id.unwrap_or(Value::Null));
        rebuilt.insert("members".into(), Value::Map(expected));
        expected = rebuilt;
        let mut actual = Map::new();
        actual.insert("Error".into(), Value::Map(error.error));
        let id = error.request_id.map_or(Value::Null, Value::String);
        actual.insert("RequestId".into(), id);
        actual.insert("members".into(), Value::Map(error.members));
        Value::Map(actual)
    };
    if actual == Value::Map(expected.clone()) {
        Ok(())
    } else {
        Err(format!("parsed {actual:?}\nexpected {expected:?}").into())
    }
}

#[test]
fn botocore_s_rest_xml_requests_and_replies_over_the_fakes_match() -> Result<()> {
    let model = fakes_model()?;
    let fakes =
        read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/rest_xml_suite/fakes.json"))?;
    let mut failed = Vec::new();
    let mut compared = 0;
    for (kind, check) in [
        (
            "requests",
            fake_request as fn(&Model, &JsonDoc) -> Result<()>,
        ),
        ("responses", fake_response),
    ] {
        for row in fakes[kind].as_array().ok_or(kind)? {
            compared += 1;
            if let Err(error) = check(&model, row) {
                failed.push(format!("{kind} {}: {error}", row["id"]));
            }
        }
    }
    let report = format!(
        "fakes/{FILE}: compared {compared} failed {}\n",
        failed.len()
    );
    std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes())?;
    assert!(compared > 40, "only {compared} fakes rows");
    assert!(
        failed.is_empty(),
        "{} failed:\n{}",
        failed.len(),
        failed.join("\n")
    );
    Ok(())
}

/// The body comparison is no looser than C14N where it matters: each of
/// these differs from the first body and must not compare equal.
#[test]
fn the_body_comparison_tells_apart_what_canonical_xml_does() {
    let base: &[u8] =
        br#"<A xmlns="https://a/" k="1"><B>t</B><C xmlns:p="https://p/" p:x="2"/></A>"#;
    let same: &[&[u8]] = &[
        br#"<A k="1" xmlns="https://a/">
             <B> t </B><C xmlns:p="https://p/" p:x="2"></C></A>"#,
        concat!(
            r#"<?xml version="1.0"?><A xmlns="https://a/" k="1"><B>t</B>"#,
            r#"<C p:x="2" xmlns:p="https://p/"/></A>"#
        )
        .as_bytes(),
    ];
    for body in same {
        assert!(same_body(base, body), "{}", String::from_utf8_lossy(body));
    }
    let different: &[&[u8]] = &[
        // An attribute's value.
        br#"<A xmlns="https://a/" k="2"><B>t</B><C xmlns:p="https://p/" p:x="2"/></A>"#,
        // A missing attribute.
        br#"<A xmlns="https://a/"><B>t</B><C xmlns:p="https://p/" p:x="2"/></A>"#,
        // Text.
        br#"<A xmlns="https://a/" k="1"><B>u</B><C xmlns:p="https://p/" p:x="2"/></A>"#,
        // Children reordered.
        br#"<A xmlns="https://a/" k="1"><C xmlns:p="https://p/" p:x="2"/><B>t</B></A>"#,
        // An extra child.
        br#"<A xmlns="https://a/" k="1"><B>t</B><B>t</B><C xmlns:p="https://p/" p:x="2"/></A>"#,
        // The default namespace.
        br#"<A xmlns="https://b/" k="1"><B>t</B><C xmlns:p="https://p/" p:x="2"/></A>"#,
        // No namespace.
        br#"<A k="1"><B>t</B><C xmlns:p="https://p/" p:x="2"/></A>"#,
        // A prefix renamed, same URI.
        br#"<A xmlns="https://a/" k="1"><B>t</B><C xmlns:q="https://p/" q:x="2"/></A>"#,
        // A tag renamed.
        br#"<A xmlns="https://a/" k="1"><D>t</D><C xmlns:p="https://p/" p:x="2"/></A>"#,
        // Empty text against none is the same; a child against text is not.
        br#"<A xmlns="https://a/" k="1"><B><t/></B><C xmlns:p="https://p/" p:x="2"/></A>"#,
    ];
    for body in different {
        assert!(!same_body(base, body), "{}", String::from_utf8_lossy(body));
    }
}

/// A body that is not XML on either side compares as bytes, as
/// `ET.ParseError` makes `_assert_xml_bodies` compare it.
#[test]
fn a_body_that_is_not_xml_compares_as_bytes() {
    assert!(same_body(b"plain", b"plain"));
    assert!(!same_body(b"plain", b"plain "));
    assert!(!same_body(b"<A/>", b"<A>"));
    assert!(same_body(b"", b""));
    assert!(!same_body(b"", b"<A/>"));
}

/// The suite reaches rest-xml through the built handle, and admission's
/// protocols hold it too.
#[test]
fn rest_xml_is_built_and_admitted() {
    assert!(built_protocol_named("rest-xml").is_some());
    assert!(client::testing::facade::protocol_named("rest-xml").is_some());
}
