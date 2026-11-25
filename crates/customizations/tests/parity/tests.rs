//! The differential test: for every case `tests/parity/oracle.py` captured
//! from botocore, two layers must give the same bytes.
//!
//! - Layer A: the Rust validator, serializer and parsers (with botocore's
//!   default parsers and the CLI's two) and `format`'s renderings.
//! - Layer B: the whole call through `Client::call` with the CLI's
//!   customizations (`calls`): every request sent, signature included, and
//!   the outcome.
//!
//! `PARITY_VECTORS` names the capture; `cargo xtask parity` sets it.
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

mod calls;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// `json.dumps(value, indent=4, default=json_encoder, ensure_ascii=False)`,
/// as text: the parity cases' values print UTF-8 only.
fn dumped(value: &Value) -> String {
    let options = model::pyjson::Options {
        ensure_ascii: false,
        ..model::pyjson::Options::indented(4)
    };
    let text = model::pyjson::dumps_wtf8(value, &options, Some(&format::json::json_encoder));
    String::from_utf8_lossy(&text.unwrap_or_default()).into_owned()
}

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A zoned ISO 8601 case value's instant: its wall time less its offset.
fn instant(text: &str) -> Result<Timestamp> {
    let zoned = Timestamp::parse_iso8601_zoned(text).ok_or("datetime")?;
    let offset = i64::from(zoned.offset_seconds.ok_or("a naive datetime")?);
    let wall = Timestamp::from_civil(zoned.civil).ok_or("datetime range")?;
    Ok(Timestamp::from_unix_micros(wall.unix_micros() - offset * 1_000_000).ok_or("range")?)
}

/// The case encoding `oracle.py` decodes: `{"$blob": base64}` and
/// `{"$datetime": ISO 8601}`.
fn decode(value: Value) -> Result<Value> {
    Ok(match value {
        Value::Map(map)
            if map.len() == 1 && (map.contains_key("$blob") || map.contains_key("$datetime")) =>
        {
            match map.iter().next().map(|(key, value)| (key.as_str(), value)) {
                Some((Some("$blob"), Value::String(text))) => {
                    Value::Blob(pytext::base64::decode(text)?)
                }
                Some((Some("$datetime"), Value::String(text))) => Value::Timestamp(instant(text)?),
                _ => return Err("tag".into()),
            }
        }
        Value::Map(map) => {
            let mut decoded = Map::new();
            for (key, item) in map {
                decoded.insert_key(key, decode(item)?);
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
        "body": pytext::base64::encode(&request.body),
        "host_prefix": request.host_prefix,
    })
}

fn response(case: &Json) -> Option<HttpResponse> {
    case.get("response").map(canned)
}

/// One canned response of a case, as `oracle.py` builds it.
fn canned(canned: &Json) -> HttpResponse {
    let headers = canned["headers"].as_object().map(|headers| {
        Headers::from_pairs(
            headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str().unwrap_or_default())),
        )
    });
    HttpResponse {
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
    }
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
                json!({ "error": text, "details": dumped(&details(error)) })
            }
            Err(error) => json!({ "parse_error": error.message }),
        };
    }
    let parsed = match protocol.parse(model, operation, response, options) {
        Ok(parsed) => parsed,
        Err(error) => return json!({ "parse_error": error.message }),
    };
    let mut result = json!({ "parsed": dumped(&parsed) });
    if name != "botocore" {
        result["json"] = rendered(&parsed, OutputFormat::Json);
        result["text"] = rendered(&parsed, OutputFormat::Text);
    }
    result
}

/// What `format` prints to stdout for `value`, as text: the parity cases
/// print UTF-8 only, which a lone surrogate's byte would not be.
fn rendered(value: &Value, format: OutputFormat) -> Json {
    let stdout = pytext::codec::Errors::SurrogateEscape;
    match format::render(value.clone(), format, stdout) {
        Ok(text) => Json::String(String::from_utf8_lossy(&text).into_owned()),
        Err(partial) => json!({
            "written": String::from_utf8_lossy(&partial.written),
            "error": partial.error.message,
        }),
    }
}

fn details(error: ServiceError) -> Value {
    let mut details = Map::new();
    details.insert("Error".into(), Value::Map(error.error));
    for (key, value) in error.members {
        details.insert_key(key, value);
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

fn identity_blob(text: &Value) -> std::result::Result<Value, ParseError> {
    Ok(text.clone())
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

/// Whether the declared `path` covers a difference: the path itself, or
/// anything under it.
fn covers(path: &str, text: &str) -> bool {
    text.starts_with(&format!("{path}:")) || text.starts_with(&format!("{path}."))
}

/// The differences of one case once its declared ones are set aside; a
/// declared difference that no longer differs is itself a failure.
fn undeclared(
    name: &str,
    found: Vec<String>,
    declared: &[(String, String)],
    layer: &str,
) -> Vec<String> {
    let paths: Vec<&str> = declared
        .iter()
        .filter(|(case, path)| {
            case == name && (path == layer || path.starts_with(&format!("{layer}.")))
        })
        .map(|(_, path)| path.as_str())
        .collect();
    let mut left: Vec<String> = found
        .iter()
        .filter(|text| !paths.iter().any(|path| covers(path, text)))
        .cloned()
        .collect();
    for path in paths {
        if !found.iter().any(|text| covers(path, text)) {
            left.push(format!(
                "{path}: declared in differences.json but identical now: remove it"
            ));
        }
    }
    left
}

/// The capture `cargo xtask parity` made, with its filters.
struct Selection {
    vectors: Vec<Json>,
    filter: String,
    services: Vec<String>,
}

impl Selection {
    fn from_environment() -> Result<Self> {
        let path = std::env::var_os("PARITY_VECTORS")
            .ok_or("PARITY_VECTORS is unset: run `cargo xtask parity`")?;
        let services = std::env::var("PARITY_SERVICE").unwrap_or_default();
        Ok(Self {
            vectors: serde_json::from_slice(&std::fs::read(path)?)?,
            filter: std::env::var("PARITY_FILTER").unwrap_or_default(),
            services: services
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
        })
    }
    fn selects(&self, case: &Json) -> bool {
        let name = case["name"].as_str().unwrap_or_default();
        let service = case["service"].as_str().unwrap_or_default();
        name.contains(self.filter.as_str())
            && (self.services.is_empty() || self.services.iter().any(|s| s == service))
    }
}

/// One case's verdict at one layer.
enum Judged {
    Identical,
    /// Every difference is declared in `differences.json`.
    Declared,
    /// The undeclared differences, as the failure prints them.
    Differs(String),
}

fn judge(
    key: &str,
    vector: &Json,
    compute: fn(&Json) -> Result<Json>,
    expected: fn(&Json) -> Json,
    declared: &[(String, String)],
) -> Judged {
    let case = &vector["case"];
    let name = case["name"].as_str().unwrap_or_default();
    let actual =
        compute(case).unwrap_or_else(|error| json!({ "harness_error": error.to_string() }));
    let mut found = Vec::new();
    differences(key, &expected(&vector[key]), &actual, &mut found);
    let differed = !found.is_empty();
    let left = undeclared(name, found, declared, key);
    match (differed, left.is_empty()) {
        (false, _) => Judged::Identical,
        (true, true) => Judged::Declared,
        (true, false) => Judged::Differs(format!(
            "case {name} ({}):\n{}",
            vector["file"],
            left.join("\n")
        )),
    }
}

/// Compares layer `key` (`a` or `b`) of every selected case with what
/// `compute` gives for it, prints the per-service table, and fails with
/// every undeclared difference.
fn compare(
    key: &str,
    compute: fn(&Json) -> Result<Json>,
    expected: fn(&Json) -> Json,
) -> Result<()> {
    let selection = Selection::from_environment()?;
    let declared = declared()?;
    let (mut compared, mut failures) = (0, Vec::new());
    // Per service: cases, identical, within declared differences.
    let mut per_service: std::collections::BTreeMap<String, [usize; 3]> = Default::default();
    for vector in selection
        .vectors
        .iter()
        .filter(|v| selection.selects(&v["case"]))
    {
        compared += 1;
        let service = vector["case"]["service"].as_str().unwrap_or_default();
        let counts = per_service.entry(service.to_owned()).or_default();
        counts[0] += 1;
        match judge(key, vector, compute, expected, &declared) {
            Judged::Identical => counts[1] += 1,
            Judged::Declared => counts[2] += 1,
            Judged::Differs(failure) => failures.push(failure),
        }
    }
    let total = |index: usize| {
        per_service
            .values()
            .map(|counts| counts[index])
            .sum::<usize>()
    };
    let mut report = format!(
        "parity layer {}: {compared} cases, {} identical, {} within declared differences, \
         {} differ\n",
        key.to_uppercase(),
        total(1),
        total(2),
        failures.len()
    );
    for (service, [cases, same, declared]) in &per_service {
        report.push_str(&format!(
            "  {service:<10} {cases:>3} cases {same:>3} identical {declared:>2} declared\n"
        ));
    }
    std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes())?;
    assert!(
        compared > 0,
        "no case matched PARITY_FILTER={:?}",
        selection.filter
    );
    assert!(
        failures.is_empty(),
        "{} of {compared} cases differ:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    Ok(())
}

/// Runs under `cargo xtask parity`, which captures the vectors first.
#[test]
#[ignore = "runs under cargo xtask parity"]
fn parity_layer_a_matches_botocore() -> Result<()> {
    compare("a", layer_a, Json::clone)
}

/// Runs under `cargo xtask parity`, which captures the vectors first.
#[test]
#[ignore = "runs under cargo xtask parity"]
fn parity_layer_b_matches_botocore() -> Result<()> {
    compare("b", calls::layer_b, calls::expected)
}
