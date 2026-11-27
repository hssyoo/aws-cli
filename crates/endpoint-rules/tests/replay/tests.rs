use endpoint_rules::{EvalError, EvalValue, Library, Outcome, Params, Partitions, Program};
use serde_json::{Map, Value};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

type Error = Box<dyn std::error::Error>;

/// Cases whose expected value disagrees with the engine, by directory and
/// case index, with why.
const DISAGREEMENTS: &[(&str, usize, &str)] = &[];

/// Directories with tests but no rule set.
const NO_RULE_SET: &[&str] = &["sms-voice"];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The rule set botocore's loader picks: the latest API version's.
fn rule_set(service: &str) -> Result<Option<PathBuf>, Error> {
    let data = repo().join("aws-cli/awscli/botocore/data").join(service);
    let mut versions = Vec::new();
    if !data.is_dir() {
        return Ok(None);
    }
    for entry in std::fs::read_dir(&data)? {
        let path = entry?.path().join("endpoint-rule-set-1.json");
        if path.is_file() {
            versions.push(path);
        }
    }
    versions.sort();
    Ok(versions.pop())
}

fn value(json: &Value) -> Option<EvalValue> {
    Some(match json {
        Value::String(text) => EvalValue::str(text),
        Value::Bool(flag) => EvalValue::Bool(*flag),
        Value::Array(items) => {
            EvalValue::Array(items.iter().map(value).collect::<Option<Vec<_>>>()?.into())
        }
        _ => return None,
    })
}

fn params(program: &Program, case: &Value) -> Result<Params, String> {
    let mut params = Params::new(program);
    let empty = Map::new();
    for (name, json) in case
        .get("params")
        .and_then(Value::as_object)
        .unwrap_or(&empty)
    {
        let value = value(json).ok_or_else(|| format!("param {name}: {json}"))?;
        // botocore passes a name the rule set does not declare into scope,
        // where no rule reads it.
        if let Some(param) = program.param(name) {
            params.set(param.slot(), value);
        }
    }
    Ok(params)
}

fn headers(expected: Option<&Value>) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = expected
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, values)| {
            let values = values.as_array().into_iter().flatten();
            let values = values.map(|v| v.as_str().unwrap_or_default().to_owned());
            (name.clone(), values.collect())
        })
        .collect();
    out.sort();
    out
}

/// Why `got` is not what `expect` describes, if it is not.
fn mismatch(got: &Result<Outcome, EvalError>, expect: &Value) -> Option<String> {
    match (got, expect.get("endpoint"), expect.get("error")) {
        (Ok(Outcome::Endpoint(endpoint)), Some(want), None) => {
            let empty = Value::Object(Map::new());
            let mut got_headers = endpoint.headers.clone();
            got_headers.sort();
            let same = want.get("url").and_then(Value::as_str) == Some(endpoint.url.as_str())
                && *want.get("properties").unwrap_or(&empty) == endpoint.properties
                && headers(want.get("headers")) == got_headers;
            (!same).then(|| format!("got {endpoint:?}, want {want}"))
        }
        (Ok(Outcome::Error(message)), None, Some(want)) => {
            (want.as_str() != Some(message)).then(|| format!("got error {message:?}, want {want}"))
        }
        (got, _, _) => Some(format!("got {got:?}, want {expect}")),
    }
}

struct Tally {
    partitions: Partitions,
    report: String,
    dirs: usize,
    cases: usize,
    failures: Vec<String>,
    disagreements_seen: Vec<(String, usize)>,
}

impl Tally {
    fn replay(&mut self, service: &str, program: &Program) -> Result<(), Error> {
        let tests = repo()
            .join("aws-cli/tests/functional/botocore/endpoint-rules")
            .join(service);
        let tests: Value =
            serde_json::from_slice(&std::fs::read(tests.join("endpoint-tests-1.json"))?)?;
        let cases = tests
            .get("testCases")
            .and_then(Value::as_array)
            .ok_or("no testCases")?;
        let library = Library {
            partitions: &self.partitions,
        };
        let mut passed = 0;
        for (index, case) in cases.iter().enumerate() {
            let expect = case.get("expect").ok_or("no expect")?;
            let got = params(program, case).map(|args| program.evaluate(&args, &library))?;
            let known = DISAGREEMENTS
                .iter()
                .any(|(s, i, _)| *s == service && *i == index);
            match (mismatch(&got, expect), known) {
                (None, false) => passed += 1,
                (Some(_), true) => self.disagreements_seen.push((service.to_owned(), index)),
                (None, true) => self
                    .failures
                    .push(format!("{service}#{index}: listed but agrees")),
                (Some(why), false) => self.failures.push(format!("{service}#{index}: {why}")),
            }
        }
        self.dirs += 1;
        self.cases += cases.len();
        writeln!(self.report, "{service}: {passed}/{} passed", cases.len())?;
        Ok(())
    }
}

/// Every directory's rule set decodes, and its cases replay to their
/// expectations, but the listed disagreements.
#[test]
fn the_suite_replays() -> Result<(), Error> {
    let root = repo().join("aws-cli/tests/functional/botocore/endpoint-rules");
    let mut services: Vec<String> = std::fs::read_dir(root)?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, std::io::Error>>()?;
    services.sort();
    assert_eq!(services.len(), 437, "the suite's directories");
    let partitions = repo().join("aws-cli/awscli/botocore/data/partitions.json");
    let mut tally = Tally {
        partitions: Partitions::decode(&std::fs::read(partitions)?)?,
        report: String::new(),
        dirs: 0,
        cases: 0,
        failures: Vec::new(),
        disagreements_seen: Vec::new(),
    };
    for service in &services {
        let Some(path) = rule_set(service)? else {
            assert!(
                NO_RULE_SET.contains(&service.as_str()),
                "{service} has no rule set"
            );
            writeln!(tally.report, "{service}: no rule set")?;
            continue;
        };
        match Program::decode(&std::fs::read(path)?) {
            Ok(program) => tally.replay(service, &program)?,
            Err(error) => tally
                .failures
                .push(format!("{service}: {}", error.detail())),
        }
    }
    writeln!(
        tally.report,
        "total: {} directories replayed, {} cases, {} listed disagreements",
        tally.dirs,
        tally.cases,
        tally.disagreements_seen.len()
    )?;
    std::fs::write(
        Path::new(env!("CARGO_TARGET_TMPDIR")).join("endpoint-rules-replay.txt"),
        &tally.report,
    )?;
    assert!(
        tally.failures.is_empty(),
        "{} failures:\n{}",
        tally.failures.len(),
        tally.failures.join("\n")
    );
    assert_eq!(
        tally.dirs,
        services.len() - NO_RULE_SET.len(),
        "directories replayed"
    );
    assert_eq!(tally.disagreements_seen.len(), DISAGREEMENTS.len());
    Ok(())
}

/// The comparison is the oracle: each kind of difference is a mismatch.
#[test]
fn the_comparison_catches_each_difference() -> Result<(), Error> {
    let endpoint = |url: &str, properties: Value, headers: &[(&str, &str)]| {
        Ok(Outcome::Endpoint(endpoint_rules::Endpoint {
            url: url.to_owned(),
            properties,
            headers: headers
                .iter()
                .map(|(n, v)| ((*n).to_owned(), vec![(*v).to_owned()]))
                .collect(),
        }))
    };
    let want = serde_json::json!({"endpoint": {"url": "https://a", "properties": {"p": 1},
                                               "headers": {"h": ["v"]}}});
    let props = || serde_json::json!({"p": 1});
    assert_eq!(
        mismatch(&endpoint("https://a", props(), &[("h", "v")]), &want),
        None
    );
    for got in [
        endpoint("https://b", props(), &[("h", "v")]),
        endpoint("https://a", serde_json::json!({}), &[("h", "v")]),
        endpoint(
            "https://a",
            serde_json::json!({"p": 1, "q": 2}),
            &[("h", "v")],
        ),
        endpoint("https://a", props(), &[]),
        endpoint("https://a", props(), &[("h", "w")]),
        Ok(Outcome::Error("https://a".to_owned())),
        Err(program_error()?),
    ] {
        assert!(mismatch(&got, &want).is_some(), "{got:?}");
    }
    let error = serde_json::json!({"error": "bad"});
    assert_eq!(
        mismatch(&Ok(Outcome::Error("bad".to_owned())), &error),
        None
    );
    assert!(mismatch(&Ok(Outcome::Error("worse".to_owned())), &error).is_some());
    assert!(mismatch(&endpoint("https://a", props(), &[]), &error).is_some());
    let neither = serde_json::json!({});
    assert!(mismatch(&Ok(Outcome::Error("bad".to_owned())), &neither).is_some());
    let no_url = serde_json::json!({"endpoint": {}});
    assert!(mismatch(&endpoint("", serde_json::json!({}), &[]), &no_url).is_some());
    Ok(())
}

/// An evaluation error, from a rule set whose only rule needs an unset
/// parameter.
fn program_error() -> Result<EvalError, Error> {
    let rules = br#"{"version": "1.0", "parameters": {"R": {"type": "String", "required": true}},
                     "rules": [{"type": "error", "conditions": [], "error": "x"}]}"#;
    let partitions = Partitions::empty();
    let library = Library {
        partitions: &partitions,
    };
    match Program::decode(rules).map(|p| p.evaluate(&Params::new(&p), &library)) {
        Ok(Err(error)) => Ok(error),
        other => Err(format!("{other:?}").into()),
    }
}
