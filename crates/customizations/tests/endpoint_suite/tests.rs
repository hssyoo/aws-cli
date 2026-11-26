//! Every case of `aws-cli/tests/functional/botocore/endpoint-rules/*/
//! endpoint-tests-1.json`, replayed as `test_endpoint_rulesets.py` replays
//! it: a case's `params` straight to the rules engine
//! (`test_endpoint_provider_test_cases_*`), and each of its
//! `operationInputs` through the client's provider parameters with the
//! input's built-ins alone (`test_end_to_end_test_cases_*`). Each is
//! compared with the expectation: the url, the properties and the headers
//! (by count: the engine reports only how many), or the error text.
//!
//! The cases that differ are `differences.tsv`, one line per differing
//! field: service, route (`provider`, or `e2e.<input index>`), the case's
//! index in its file, the field and what the path gave. Its header records
//! how many cases and inputs ran. The run must give exactly that file.
//! Each run writes what it gave, and the per-service pass counts, to
//! `CARGO_TARGET_TMPDIR`.
use client::testing::facade::{
    EndpointOutcome, ParamValue, RuleSet, endpoint_case, load_model, resolve_case, rule_set,
};
use model::source::{DirSource, Kind, Source};
use model::value::Value;
use serde_json::Value as JsonDoc;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cases_dir() -> PathBuf {
    repository().join("aws-cli/tests/functional/botocore/endpoint-rules")
}

fn baseline_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/endpoint_suite/differences.tsv")
}

/// `ALL_SERVICES`: the services with a rule set, sorted. A test file
/// without one (`sms-voice`'s) is never read, as botocore never reads it.
fn suite_services(source: &DirSource) -> Result<Vec<String>> {
    let mut services = source.services(Kind::EndpointRuleSet)?;
    services.sort();
    Ok(services)
}

/// A rule parameter from a case's JSON value; `None` for a kind no rule
/// set declares.
fn param(value: &JsonDoc) -> Option<ParamValue> {
    match value {
        JsonDoc::String(text) => Some(ParamValue::String(text.clone())),
        JsonDoc::Bool(flag) => Some(ParamValue::Boolean(*flag)),
        JsonDoc::Array(items) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .map(ParamValue::StringArray),
        _ => None,
    }
}

/// A case's parameters object, in its order.
fn params(object: Option<&JsonDoc>) -> std::result::Result<Vec<(String, ParamValue)>, String> {
    let Some(object) = object else {
        return Ok(Vec::new());
    };
    let object = object
        .as_object()
        .ok_or("parameters that are not an object")?;
    object
        .iter()
        .map(|(name, value)| {
            param(value)
                .map(|value| (name.clone(), value))
                .ok_or_else(|| format!("parameter {name} of an unknown kind: {value}"))
        })
        .collect()
}

fn borrowed(params: &[(String, ParamValue)]) -> Vec<(&str, ParamValue)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.clone()))
        .collect()
}

/// The fields where `outcome` differs from `expect`, each as
/// (field, what the path gave).
fn compare(expect: &JsonDoc, outcome: &EndpointOutcome) -> Vec<(&'static str, String)> {
    let mut differences = Vec::new();
    match (expect.get("endpoint"), expect.get("error"), outcome) {
        (
            Some(endpoint),
            None,
            EndpointOutcome::Endpoint {
                url,
                properties,
                headers,
            },
        ) => {
            if endpoint.get("url").and_then(JsonDoc::as_str) != Some(url.as_str()) {
                differences.push(("url", url.clone()));
            }
            let empty = JsonDoc::Object(serde_json::Map::new());
            let expected = endpoint.get("properties").unwrap_or(&empty);
            match serde_json::from_str::<JsonDoc>(properties) {
                Ok(actual) if actual == *expected => {}
                Ok(actual) => differences.push(("properties", actual.to_string())),
                Err(_) => differences.push(("properties", properties.clone())),
            }
            let expected = endpoint
                .get("headers")
                .and_then(JsonDoc::as_object)
                .map_or(0, serde_json::Map::len);
            if expected != *headers {
                differences.push(("headers", headers.to_string()));
            }
        }
        (Some(_), None, EndpointOutcome::Error(text)) => differences.push(("error", text.clone())),
        (None, Some(_), EndpointOutcome::Endpoint { url, .. }) => {
            differences.push(("endpoint", url.clone()));
        }
        (None, Some(expected), EndpointOutcome::Error(text)) => {
            if expected.as_str() != Some(text.as_str()) {
                differences.push(("error", text.clone()));
            }
        }
        _ => differences.push(("expect", "neither one endpoint nor one error".to_owned())),
    }
    differences
}

/// botocore validates these bucket names before endpoint resolution, so
/// `iter_e2e_test_cases_that_produce` skips them.
fn skipped_input(input: &JsonDoc) -> bool {
    let bucket = input["operationParams"]["Bucket"].as_str();
    matches!(bucket, Some("bucket name" | "example.com#"))
}

#[derive(Default)]
struct Tally {
    cases: usize,
    inputs: usize,
    skipped: usize,
    /// Per service: (provider cases, provider differing, inputs, inputs differing).
    services: BTreeMap<String, [usize; 4]>,
    lines: Vec<String>,
}

impl Tally {
    fn record(&mut self, service: &str, route: &str, index: usize, found: &[(&str, String)]) {
        let counts = self.services.entry(service.to_owned()).or_default();
        let provider = route == "provider";
        counts[if provider { 0 } else { 2 }] += 1;
        if !found.is_empty() {
            counts[if provider { 1 } else { 3 }] += 1;
        }
        for (field, actual) in found {
            let actual = actual.replace(['\t', '\n'], " ");
            self.lines
                .push(format!("{service}\t{route}\t{index}\t{field}\t{actual}"));
        }
    }

    fn header(&self) -> String {
        format!(
            "# provider cases {}\n# e2e inputs {} (skipped {})\n",
            self.cases, self.inputs, self.skipped
        )
    }

    fn counts(&self) -> String {
        let mut text =
            String::from("service\tprovider_pass\tprovider_total\te2e_pass\te2e_total\n");
        for (service, [cases, bad, inputs, bad_inputs]) in &self.services {
            let _ = writeln!(
                text,
                "{service}\t{}\t{cases}\t{}\t{inputs}",
                cases - bad,
                inputs - bad_inputs
            );
        }
        text
    }
}

/// One service's cases into `tally`.
fn run_service(source: &dyn Source, service: &str, tally: &mut Tally) -> Result<()> {
    let file: JsonDoc = serde_json::from_slice(&std::fs::read(
        cases_dir().join(service).join("endpoint-tests-1.json"),
    )?)?;
    let cases = file["testCases"].as_array().ok_or("testCases")?;
    let loaded = load_model(source, service).and_then(|model| {
        let rules: RuleSet = rule_set(source, service, &model)?;
        Ok((model, rules))
    });
    for (index, case) in cases.iter().enumerate() {
        tally.cases += 1;
        let expect = &case["expect"];
        let found = match (&loaded, params(case.get("params"))) {
            (Err(error), _) => vec![("load", error.clone())],
            (_, Err(error)) => vec![("input", error)],
            (Ok((_, rules)), Ok(params)) => {
                compare(expect, &resolve_case(rules, &borrowed(&params)))
            }
        };
        tally.record(service, "provider", index, &found);
        let inputs = case
            .get("operationInputs")
            .and_then(JsonDoc::as_array)
            .map_or(&[][..], Vec::as_slice);
        for (k, input) in inputs.iter().enumerate() {
            if skipped_input(input) {
                tally.skipped += 1;
                continue;
            }
            tally.inputs += 1;
            let found = match (&loaded, params(input.get("builtInParams"))) {
                (Err(error), _) => vec![("load", error.clone())],
                (_, Err(error)) => vec![("input", error)],
                (Ok((model, rules)), Ok(builtins)) => {
                    let operation = input["operationName"].as_str().unwrap_or_default();
                    let empty = JsonDoc::Object(serde_json::Map::new());
                    let op_params = input.get("operationParams").unwrap_or(&empty);
                    let value = Value::from_json(op_params.to_string().as_bytes())?;
                    match endpoint_case(rules, model, operation, &value, &borrowed(&builtins)) {
                        Ok(outcome) => compare(expect, &outcome),
                        Err(error) => vec![("operation", error)],
                    }
                }
            };
            tally.record(service, &format!("e2e.{k}"), index, &found);
        }
    }
    Ok(())
}

fn run_suite() -> Result<Tally> {
    let source = DirSource::new(repository().join("aws-cli/awscli/botocore/data"));
    let mut tally = Tally::default();
    for service in suite_services(&source)? {
        run_service(&source, &service, &mut tally)?;
    }
    Ok(tally)
}

/// The run's text against the recorded baseline: `None` when equal, else
/// the lines only one of them has.
fn baseline_difference(recorded: &str, actual: &str) -> Option<String> {
    if recorded == actual {
        return None;
    }
    let lines = |text: &str| text.lines().map(str::to_owned).collect::<Vec<_>>();
    let (recorded, actual) = (lines(recorded), lines(actual));
    let mut text = String::new();
    for line in recorded.iter().filter(|line| !actual.contains(line)) {
        let _ = writeln!(text, "- {line}");
    }
    for line in actual.iter().filter(|line| !recorded.contains(line)) {
        let _ = writeln!(text, "+ {line}");
    }
    if text.is_empty() {
        text.push_str("same lines, different order or repetition\n");
    }
    Some(text)
}

#[test]
fn every_case_matches_the_recorded_baseline() -> Result<()> {
    let tally = run_suite()?;
    let mut actual = tally.header();
    for line in &tally.lines {
        actual.push_str(line);
        actual.push('\n');
    }
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    std::fs::write(out.join("endpoint_suite-counts.tsv"), tally.counts())?;
    let written = out.join("endpoint_suite-differences.tsv");
    std::fs::write(&written, &actual)?;
    let recorded = std::fs::read_to_string(baseline_path())?;
    if let Some(difference) = baseline_difference(&recorded, &actual) {
        let shown: String = difference.lines().take(40).collect::<Vec<_>>().join("\n");
        return Err(format!(
            "the endpoint suite differs from {}; the run is in {}:\n{shown}",
            baseline_path().display(),
            written.display()
        )
        .into());
    }
    assert!(tally.cases > 0 && tally.inputs > 0, "no case ran");
    Ok(())
}

/// `test_all_endpoint_tests_exist`: every service with a rule set has a
/// test file with cases.
#[test]
fn every_rule_set_has_its_cases() -> Result<()> {
    let source = DirSource::new(repository().join("aws-cli/awscli/botocore/data"));
    let services = suite_services(&source)?;
    assert!(
        services.len() > 400,
        "{} services with a rule set",
        services.len()
    );
    for service in services {
        let path = cases_dir().join(&service).join("endpoint-tests-1.json");
        let file: JsonDoc = serde_json::from_slice(&std::fs::read(&path)?)?;
        let cases = file["testCases"].as_array().map_or(0, Vec::len);
        assert!(cases > 0, "{service} has no cases");
    }
    Ok(())
}

// ------------------------------------------------- the comparison, attacked

fn endpoint(url: &str, properties: &str, headers: usize) -> EndpointOutcome {
    EndpointOutcome::Endpoint {
        url: url.to_owned(),
        properties: properties.to_owned(),
        headers,
    }
}

fn expect(text: &str) -> JsonDoc {
    serde_json::from_str(text).unwrap_or(JsonDoc::Null)
}

#[test]
fn an_equal_endpoint_has_no_difference() {
    let expected = expect(
        r#"{"endpoint": {"url": "https://a",
            "properties": {"authSchemes": [{"name": "sigv4", "x": 1}]},
            "headers": {"h": ["v"]}}}"#,
    );
    let actual = endpoint(
        "https://a",
        r#"{"authSchemes":[{"x":1,"name":"sigv4"}]}"#,
        1,
    );
    assert_eq!(compare(&expected, &actual), vec![]);
    let bare = expect(r#"{"endpoint": {"url": "https://a"}}"#);
    assert_eq!(compare(&bare, &endpoint("https://a", "{}", 0)), vec![]);
}

#[test]
fn each_differing_endpoint_field_is_reported() {
    let expected = expect(r#"{"endpoint": {"url": "https://a", "properties": {"p": [1, 2]}}}"#);
    let found = compare(&expected, &endpoint("https://a/", r#"{"p":[2,1]}"#, 1));
    let fields: Vec<&str> = found.iter().map(|(field, _)| *field).collect();
    assert_eq!(fields, ["url", "properties", "headers"]);
    // Missing properties are `{}`, and extra ones differ.
    let bare = expect(r#"{"endpoint": {"url": "https://a"}}"#);
    assert_eq!(
        compare(&bare, &endpoint("https://a", r#"{"a":1}"#, 0)).len(),
        1
    );
    assert_eq!(
        compare(&bare, &endpoint("https://a", "not json", 0)).len(),
        1
    );
    // Expected headers the engine did not give.
    let headers = expect(r#"{"endpoint": {"url": "https://a", "headers": {"h": ["v"], "i": []}}}"#);
    assert_eq!(
        compare(&headers, &endpoint("https://a", "{}", 1)),
        vec![("headers", "1".to_owned())]
    );
}

#[test]
fn an_error_for_an_endpoint_and_an_endpoint_for_an_error_are_reported() {
    let wants_endpoint = expect(r#"{"endpoint": {"url": "https://a"}}"#);
    let error = EndpointOutcome::Error("boom".to_owned());
    assert_eq!(
        compare(&wants_endpoint, &error),
        vec![("error", "boom".to_owned())]
    );
    let wants_error = expect(r#"{"error": "boom"}"#);
    assert_eq!(compare(&wants_error, &error), vec![]);
    assert_eq!(
        compare(&wants_error, &endpoint("https://a", "{}", 0)).len(),
        1
    );
    let other = EndpointOutcome::Error("Boom".to_owned());
    assert_eq!(
        compare(&wants_error, &other),
        vec![("error", "Boom".to_owned())]
    );
    // An expectation with both, or neither, never passes.
    let both = expect(r#"{"endpoint": {"url": "https://a"}, "error": "boom"}"#);
    assert_eq!(compare(&both, &endpoint("https://a", "{}", 0)).len(), 1);
    assert_eq!(compare(&expect("{}"), &error).len(), 1);
}

#[test]
fn a_parameter_of_an_unknown_kind_is_a_difference_not_a_skip() {
    let object = expect(r#"{"Region": "us-east-1", "N": 1}"#);
    assert!(params(Some(&object)).is_err());
    let mixed = expect(r#"{"A": ["x", 1]}"#);
    assert!(params(Some(&mixed)).is_err());
    let good = expect(r#"{"B": true, "A": ["x"], "R": "r"}"#);
    let names: Vec<String> = params(Some(&good)).map_or_else(
        |_| vec![],
        |list| list.into_iter().map(|(name, _)| name).collect(),
    );
    assert_eq!(names.len(), 3);
}

#[test]
fn a_baseline_that_gains_loses_or_reorders_lines_differs() {
    let recorded = "# provider cases 2\na\tprovider\t0\turl\tx\n";
    assert_eq!(baseline_difference(recorded, recorded), None);
    assert!(baseline_difference(recorded, "# provider cases 2\n").is_some());
    assert!(
        baseline_difference(recorded, &format!("{recorded}b\tprovider\t1\turl\ty\n")).is_some()
    );
    assert!(
        baseline_difference(recorded, "# provider cases 3\na\tprovider\t0\turl\tx\n").is_some()
    );
    let two = "a\n b\n";
    assert!(baseline_difference(two, " b\na\n").is_some());
    assert!(baseline_difference(two, "a\n b\n b\n").is_some());
}

#[test]
fn only_the_inputs_botocore_skips_are_skipped() {
    assert!(skipped_input(&expect(
        r#"{"operationParams": {"Bucket": "bucket name"}}"#
    )));
    assert!(skipped_input(&expect(
        r#"{"operationParams": {"Bucket": "example.com#"}}"#
    )));
    assert!(!skipped_input(&expect(
        r#"{"operationParams": {"Bucket": "bucket"}}"#
    )));
    assert!(!skipped_input(&expect(
        r#"{"operationParams": {"Key": "bucket name"}}"#
    )));
    assert!(!skipped_input(&expect("{}")));
}
