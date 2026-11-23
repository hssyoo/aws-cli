//! Layer A of the differential test: for every case `tests/parity/oracle.py`
//! captured from botocore, the Rust validator, serializer and parsers (with
//! botocore's default parsers and the CLI's two) and `format`'s renderings
//! must give the same bytes. `PARITY_VECTORS` names the capture; `cargo
//! xtask parity` sets it. Layer B (the full prepared request) is captured
//! too and compared once the call loop is in place.
use client::error::{ParseError, ServiceError};
use client::http::{Headers, HttpResponse};
use client::protocol::{self, ParseOptions, Protocol, Serialized};
use format::{OutputFormat, TimestampFormat, TimestampParser};
use model::shapes::{Model, Operation};
use model::source::DirSource;
use model::validate::{Validation, validate_input};
use model::value::{Map, Value};
use serde_json::{Value as Json, json};
use std::path::PathBuf;
use timestamp::Timestamp;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The case encoding `oracle.py` decodes: `{"$blob": base64}` and
/// `{"$datetime": ISO 8601}`.
fn decode(value: Value) -> Result<Value> {
    Ok(match value {
        Value::Map(map)
            if map.len() == 1 && (map.contains_key("$blob") || map.contains_key("$datetime")) =>
        {
            match map.iter().next() {
                Some(("$blob", Value::String(text))) => Value::Blob(model::base64::decode(text)?),
                Some(("$datetime", Value::String(text))) => {
                    Value::Timestamp(Timestamp::parse_iso8601(text).ok_or("datetime")?)
                }
                _ => return Err("tag".into()),
            }
        }
        Value::Map(map) => {
            let mut decoded = Map::new();
            for (key, item) in map {
                decoded.insert(key, decode(item)?);
            }
            Value::Map(decoded)
        }
        Value::List(items) => Value::List(items.into_iter().map(decode).collect::<Result<_>>()?),
        other => other,
    })
}

fn model(case: &Json) -> Result<Model> {
    let root = if case["models"] == "fake" {
        repository().join("tests/models")
    } else {
        repository().join("aws-cli/awscli/botocore/data")
    };
    let service = case["service"].as_str().ok_or("service")?;
    Ok(Model::load(&DirSource::new(root), service)?)
}

fn request_json(request: &Serialized) -> Json {
    let headers: Vec<Json> = request.headers.iter().map(|(k, v)| json!([k, v])).collect();
    json!({
        "method": request.method.as_str(),
        "path": request.path,
        "query": request.query,
        "headers": headers,
        "body": model::base64::encode(&request.body),
        "host_prefix": request.host_prefix,
    })
}

fn response(case: &Json) -> Option<HttpResponse> {
    let canned = case.get("response")?;
    let headers = canned["headers"].as_object().map(|headers| {
        Headers::from_pairs(
            headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str().unwrap_or_default())),
        )
    });
    Some(HttpResponse {
        status: canned["status"]
            .as_u64()
            .and_then(|s| u16::try_from(s).ok())
            .unwrap_or(200),
        headers: headers.unwrap_or_default(),
        body: canned["body"]
            .as_str()
            .unwrap_or_default()
            .as_bytes()
            .to_vec(),
    })
}

/// `outcome` in `oracle.py`: the parsed output with `format`'s renderings,
/// or the `ClientError` text and its details, or the parser's error.
fn outcome(
    name: &str,
    protocol: &dyn Protocol,
    model: &Model,
    operation: &Operation,
    response: &HttpResponse,
    options: &ParseOptions<'_>,
) -> Json {
    if response.status >= 300 {
        return match protocol.parse_error(model, operation, response, options) {
            Ok(error) => {
                let text = error.to_string();
                json!({ "error": text, "details": format::json::dump(&details(error)) })
            }
            Err(error) => json!({ "parse_error": error.message }),
        };
    }
    let parsed = match protocol.parse(model, operation, response, options) {
        Ok(parsed) => parsed,
        Err(error) => return json!({ "parse_error": error.message }),
    };
    let mut result = json!({ "parsed": format::json::dump(&parsed) });
    if name != "botocore" {
        result["json"] = rendered(&parsed, OutputFormat::Json);
        result["text"] = rendered(&parsed, OutputFormat::Text);
    }
    result
}

fn rendered(value: &Value, format: OutputFormat) -> Json {
    match format::render(value, format) {
        Ok(text) => Json::String(text),
        Err(partial) => json!({ "written": partial.written, "error": partial.error.message }),
    }
}

fn details(error: ServiceError) -> Value {
    let mut details = Map::new();
    details.insert("Error".into(), Value::Map(error.error));
    for (key, value) in error.members {
        details.insert(key, value);
    }
    let id = error.request_id.map_or(Value::Null, Value::String);
    details.insert("RequestId".into(), id);
    Value::Map(details)
}

fn cli_timestamp(
    format: TimestampFormat,
) -> impl Fn(&Value) -> std::result::Result<Value, ParseError> + Sync {
    let parser = TimestampParser::new(format, Some(0));
    move |value| {
        parser
            .parse(value)
            .map_err(|error| ParseError::new(format!("{error:?}")))
    }
}

fn identity_blob(text: &str) -> std::result::Result<Value, ParseError> {
    Ok(Value::String(text.to_owned()))
}

/// Layer A for one case, as the JSON `oracle.py` wrote for it.
fn layer_a(case: &Json) -> Result<Json> {
    let model = model(case)?;
    let operation = model
        .operation(case["operation"].as_str().ok_or("operation")?)
        .ok_or("no operation")?;
    let protocol = protocol::for_model(&model).ok_or("protocol not implemented")?;
    let input = decode(Value::from_serde(&case["input"]))?;
    match validate_input(&model, operation, &input) {
        Validation::Valid => {}
        Validation::Invalid(report) => return Ok(json!({ "validation": report.message() })),
        Validation::Undecided(reason) => return Ok(json!({ "undecided": format!("{reason:?}") })),
    }
    let mut vector = match protocol.serialize(&model, operation, &input) {
        Ok(request) => json!({ "request": request_json(&request) }),
        Err(error) => return Ok(json!({ "serialize_error": error.message })),
    };
    if let Some(response) = response(case) {
        let iso = cli_timestamp(TimestampFormat::Iso8601);
        let wire = cli_timestamp(TimestampFormat::Wire);
        let settings: [(&str, ParseOptions<'_>); 3] = [
            ("botocore", ParseOptions::BOTOCORE),
            (
                "iso8601",
                ParseOptions {
                    timestamp: &iso,
                    blob: &identity_blob,
                },
            ),
            (
                "wire",
                ParseOptions {
                    timestamp: &wire,
                    blob: &identity_blob,
                },
            ),
        ];
        let mut outcomes = serde_json::Map::new();
        for (name, options) in &settings {
            let result = outcome(name, protocol, &model, operation, &response, options);
            outcomes.insert((*name).to_owned(), result);
        }
        vector["outcomes"] = Json::Object(outcomes);
    }
    Ok(vector)
}

/// Every differing leaf between `expected` and `actual`, by path, with a
/// line-by-line view of differing text.
fn differences(path: &str, expected: &Json, actual: &Json, out: &mut Vec<String>) {
    match (expected, actual) {
        (Json::Object(e), Json::Object(a)) => {
            for key in e.keys().chain(a.keys().filter(|k| !e.contains_key(*k))) {
                let (left, right) = (
                    e.get(key).unwrap_or(&Json::Null),
                    a.get(key).unwrap_or(&Json::Null),
                );
                differences(&format!("{path}.{key}"), left, right, out);
            }
        }
        (Json::String(e), Json::String(a)) if e != a => out.push(line_diff(path, e, a)),
        _ if expected != actual => out.push(format!(
            "{path}:\n  botocore {expected}\n  rust     {actual}"
        )),
        _ => {}
    }
}

fn line_diff(path: &str, expected: &str, actual: &str) -> String {
    let mut text = format!("{path}:");
    let (left, right): (Vec<&str>, Vec<&str>) =
        (expected.lines().collect(), actual.lines().collect());
    for index in 0..left.len().max(right.len()) {
        let (e, a) = (left.get(index), right.get(index));
        if e != a {
            text.push_str(&format!(
                "\n  line {}:\n  - {}\n  + {}",
                index + 1,
                e.unwrap_or(&"<none>"),
                a.unwrap_or(&"<none>")
            ));
        }
    }
    if left == right {
        text.push_str(&format!("\n  - {expected:?}\n  + {actual:?}"));
    }
    text
}

/// `tests/parity/differences.json`: the declared differences, each a case
/// and a path with the reason it differs.
fn declared() -> Result<Vec<(String, String)>> {
    let ledger: Vec<Json> = serde_json::from_slice(&std::fs::read(
        repository().join("tests/parity/differences.json"),
    )?)?;
    Ok(ledger
        .iter()
        .map(|entry| {
            let field = |key: &str| entry[key].as_str().unwrap_or_default().to_owned();
            (field("case"), field("path"))
        })
        .collect())
}

/// The differences of one case once its declared ones are set aside; a
/// declared difference that no longer differs is itself a failure.
fn undeclared(name: &str, found: Vec<String>, declared: &[(String, String)]) -> Vec<String> {
    let paths: Vec<&str> = declared
        .iter()
        .filter(|(case, _)| case == name)
        .map(|(_, path)| path.as_str())
        .collect();
    let mut left: Vec<String> = found
        .iter()
        .filter(|text| {
            !paths
                .iter()
                .any(|path| text.starts_with(&format!("{path}:")))
        })
        .cloned()
        .collect();
    for path in paths {
        if !found
            .iter()
            .any(|text| text.starts_with(&format!("{path}:")))
        {
            left.push(format!(
                "{path}: declared in differences.json but identical now: remove it"
            ));
        }
    }
    left
}

/// Runs under `cargo xtask parity`, which captures the vectors first.
#[test]
#[ignore = "runs under cargo xtask parity"]
fn parity_layer_a_matches_botocore() -> Result<()> {
    let path = std::env::var_os("PARITY_VECTORS")
        .ok_or("PARITY_VECTORS is unset: run `cargo xtask parity`")?;
    let filter = std::env::var("PARITY_FILTER").unwrap_or_default();
    let services = std::env::var("PARITY_SERVICE").unwrap_or_default();
    let services: Vec<&str> = services.split(',').filter(|s| !s.is_empty()).collect();
    let vectors: Vec<Json> = serde_json::from_slice(&std::fs::read(path)?)?;
    let declared = declared()?;
    let (mut compared, mut failures) = (0, Vec::new());
    let mut per_service: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    for vector in &vectors {
        let case = &vector["case"];
        let name = case["name"].as_str().unwrap_or_default();
        let service = case["service"].as_str().unwrap_or_default();
        if !name.contains(filter.as_str()) || !(services.is_empty() || services.contains(&service))
        {
            continue;
        }
        compared += 1;
        let actual =
            layer_a(case).unwrap_or_else(|error| json!({ "harness_error": error.to_string() }));
        let mut found = Vec::new();
        differences("a", &vector["a"], &actual, &mut found);
        let found = undeclared(name, found, &declared);
        let entry = per_service
            .entry(case["service"].as_str().unwrap_or_default().to_owned())
            .or_default();
        entry.0 += 1;
        if found.is_empty() {
            entry.1 += 1;
        } else {
            failures.push(format!(
                "case {name} ({}):\n{}",
                vector["file"],
                found.join("\n")
            ));
        }
    }
    let mut report = format!(
        "parity layer A: {compared} cases, {} identical\n",
        compared - failures.len()
    );
    for (service, (cases, same)) in &per_service {
        report.push_str(&format!(
            "  {service:<10} {cases:>3} cases {same:>3} identical\n"
        ));
    }
    let layer_b = vectors.iter().filter(|v| v.get("b").is_some()).count();
    report.push_str(&format!(
        "parity layer B: {layer_b} captured, compared when the call loop lands\n"
    ));
    std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes())?;
    assert!(compared > 0, "no case matched PARITY_FILTER={filter:?}");
    assert!(
        failures.is_empty(),
        "{} of {compared} cases differ:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    Ok(())
}
