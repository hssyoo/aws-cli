//! The engine over [`Value`]s against its oracles: the JMESPath
//! compliance suite, and jmespath 1.0.1 for Python on that suite, the
//! probes in `crates/jmespath/testdata` and every expression the botocore
//! models ship (`oracle.py`).
use super::Python;
use crate::pyjson;
use crate::pyrepr::py_repr;
use crate::value::{Map, Value};
use std::path::PathBuf;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn testdata() -> PathBuf {
    repository().join("crates/jmespath/testdata")
}

/// A case: where it comes from, the expression, the input as JSON text.
struct Case {
    kind: String,
    expression: String,
    given: Option<String>,
}

/// `oracle.py`'s `decode`: tagged objects as bytes and datetimes.
fn decode(value: Value) -> Value {
    match value {
        Value::Map(map) => {
            let tag = |name: &str| map.len() == 1 && map.contains_key(name);
            if tag("$bytes") {
                let text = map
                    .get("$bytes")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                return Value::Blob(pytext::base64::decode(text).unwrap_or_default());
            }
            if tag("$datetime") {
                let seconds = match map.get("$datetime") {
                    Some(Value::Integer(seconds)) => *seconds,
                    _ => 0,
                };
                return timestamp::Timestamp::from_unix_seconds(seconds)
                    .map_or(Value::Null, Value::Timestamp);
            }
            Value::Map(
                map.into_iter()
                    .map(|(key, value)| (key, decode(value)))
                    .collect::<Map>(),
            )
        }
        Value::List(items) => Value::List(items.into_iter().map(decode).collect()),
        other => other,
    }
}

/// The native outcome, as `oracle.py` writes Python's.
fn native(expression: &str, given: &str) -> String {
    let value = match pyjson::loads(given) {
        Ok(value) => decode(value),
        Err(error) => return format!("unreadable input: {error:?}"),
    };
    match jmespath::search(&Python, expression, &value) {
        Ok(found) => py_repr(&found),
        Err(error) => format!("{}: {}", error.class(), error.message()),
    }
}

/// Each compliance case with its expected `result` or `error`.
fn compliance() -> Result<Vec<(Case, std::result::Result<Value, String>)>> {
    let mut cases = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(testdata().join("compliance"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<_>>()?;
    files.sort();
    for file in files {
        let groups: serde_json::Value = serde_json::from_slice(&std::fs::read(&file)?)?;
        for group in groups.as_array().ok_or("a compliance file is a list")? {
            let given = group["given"].to_string();
            for case in group["cases"].as_array().ok_or("cases")? {
                let expected = match case.get("error") {
                    Some(error) => Err(error.as_str().unwrap_or_default().to_owned()),
                    None => Ok(Value::from_serde(&case["result"])),
                };
                let expression = case["expression"].as_str().ok_or("expression")?.to_owned();
                let kind = format!("compliance {}", file.display());
                cases.push((
                    Case {
                        kind,
                        expression,
                        given: Some(given.clone()),
                    },
                    expected,
                ));
            }
        }
    }
    Ok(cases)
}

/// The probes: one JSON object per line, read as Python reads them.
fn probes(name: &str) -> Result<Vec<Case>> {
    let text = std::fs::read_to_string(testdata().join(name))?;
    let mut cases = Vec::new();
    for line in text.lines() {
        let Value::Map(probe) = pyjson::loads(line).map_err(|e| format!("{e:?}"))? else {
            return Err("a probe is an object".into());
        };
        let expression = probe
            .get("expression")
            .and_then(Value::as_str)
            .ok_or("expression")?;
        let given = pyjson::dumps(
            probe.get("given").ok_or("given")?,
            &pyjson::Options::default(),
            None,
        )?;
        cases.push(Case {
            kind: name.to_owned(),
            expression: expression.to_owned(),
            given: Some(given),
        });
    }
    Ok(cases)
}

/// Python's equality of a result and the suite's expectation.
fn same(a: &Value, b: &Value) -> bool {
    let number = |value: &Value| match value {
        Value::Integer(int) => Some(*int as f64),
        Value::Float(float) => Some(*float),
        _ => None,
    };
    match (a, b) {
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
        }
        (Value::Map(a), Value::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get_key(key).is_some_and(|b| same(a, b)))
        }
        _ => match (number(a), number(b)) {
            (Some(a), Some(b)) => a == b,
            _ => a == b,
        },
    }
}

/// The exception classes each of the suite's error names stands for.
fn error_classes(error: &str) -> &'static [&'static str] {
    match error {
        "syntax" => &[
            "ParseError",
            "LexerError",
            "IncompleteExpressionError",
            "EmptyExpressionError",
        ],
        "invalid-arity" => &["ArityError", "VariadictArityError"],
        "invalid-type" => &["JMESPathTypeError"],
        "unknown-function" => &["UnknownFunctionError"],
        "invalid-value" => &["ValueError"],
        _ => &[],
    }
}

#[test]
fn the_compliance_suite_passes() -> Result {
    let cases = compliance()?;
    let mut failures = Vec::new();
    for (case, expected) in &cases {
        let value =
            pyjson::loads(case.given.as_deref().unwrap_or("{}")).map_err(|e| format!("{e:?}"))?;
        let found = jmespath::search(&Python, &case.expression, &value);
        let passed = match (&found, expected) {
            (Ok(found), Ok(expected)) => same(found, expected),
            (Err(error), Err(name)) => error_classes(name).contains(&error.class()),
            _ => false,
        };
        if !passed {
            failures.push(format!(
                "{} {:?}: {found:?}, expected {expected:?}",
                case.kind, case.expression
            ));
        }
    }
    assert_eq!(cases.len(), 892, "the vendored suite has 892 cases");
    assert!(
        failures.is_empty(),
        "{} of 892 fail:\n{}",
        failures.len(),
        failures.join("\n")
    );
    Ok(())
}

/// Runs `oracle.py` on `cases` and the models' expressions: each case
/// with its input and Python's outcome.
fn oracle(cases: &[Case]) -> Result<Vec<(Case, String)>> {
    // The cases go in a file: the helper writes all of stdin before it
    // reads stdout, which the oracle fills meanwhile.
    let mut lines = String::new();
    for case in cases {
        let (kind, expression, given) = (&case.kind, &case.expression, &case.given);
        let line = serde_json::json!({"kind": kind, "expression": expression, "given": given});
        lines.push_str(&format!("{line}\n"));
    }
    let file = std::env::temp_dir().join(format!("jmespath-oracle-{}.jsonl", std::process::id()));
    std::fs::write(&file, lines)?;
    let config = serde_json::json!({
        "python": testdata().join("python"),
        "data": repository().join("aws-cli/awscli/botocore/data"),
        "cases": file,
    });
    let output = pytext::testing::python(include_str!("oracle.py"), config.to_string().as_bytes());
    std::fs::remove_file(&file)?;
    let output = output?;
    let mut rows = Vec::new();
    for line in String::from_utf8(output)?.lines() {
        let row: serde_json::Value = serde_json::from_str(line)?;
        let text = |key: &str| {
            row[key]
                .as_str()
                .map(str::to_owned)
                .ok_or(format!("oracle row lacks {key}"))
        };
        let case = Case {
            kind: text("kind")?,
            expression: text("expression")?,
            given: Some(text("given")?),
        };
        rows.push((case, text("outcome")?));
    }
    Ok(rows)
}

/// The sha256 of the vendored `RECORD`: the jmespath 1.0.1 wheel's lines
/// for its modules, which `oracle.py` checks the sources against.
const RECORD_SHA256: &str = "faa4cd7b18aaa0ff56610c301b2cfddacdfb9820f51c37f7ff998ca95bc9022a";

#[test]
fn every_outcome_is_python_s() -> Result {
    let record = std::fs::read(testdata().join("python/RECORD"))?;
    let digest = pytext::testing::python(
        "import hashlib, sys; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest(), end='')",
        &record,
    )?;
    assert_eq!(
        String::from_utf8(digest)?,
        RECORD_SHA256,
        "the vendored jmespath is not 1.0.1's"
    );
    let mut cases: Vec<Case> = compliance()?.into_iter().map(|(case, _)| case).collect();
    cases.extend(probes("probes.jsonl")?);
    cases.extend(probes("probes-native.jsonl")?);
    let rows = oracle(&cases)?;
    // Python answers each case sent, in order, before the models' own.
    assert!(
        rows.len() > cases.len(),
        "the oracle wrote {} rows",
        rows.len()
    );
    for (sent, (row, _)) in cases.iter().zip(&rows) {
        assert_eq!(
            (&sent.expression, &sent.given),
            (&row.expression, &row.given)
        );
    }
    let mut differences = Vec::new();
    let mut kinds = std::collections::BTreeMap::<&str, [usize; 2]>::new();
    for (case, python) in &rows {
        let tally = kinds
            .entry(case.kind.split(' ').next().unwrap_or_default())
            .or_default();
        tally[0] += 1;
        tally[1] += usize::from(python != "None");
        let given = case.given.as_deref().unwrap_or_default();
        let rust = native(&case.expression, given);
        if rust != *python {
            let kind = &case.kind;
            differences.push(format!(
                "{kind} {:?} on {given}\n  python: {python}\n  native: {rust}",
                case.expression
            ));
        }
    }
    // Every model expression is evaluated on `{}` and on inputs made to
    // reach it, so most of its outcomes find something.
    for (kind, least) in [("paginator", 2400), ("waiter", 150), ("ocp", 4)] {
        let [all, found] = kinds.get(kind).copied().unwrap_or_default();
        assert!(
            all > least * 3 && found * 2 > all,
            "{kind}: {found} of {all} found something"
        );
    }
    assert!(
        differences.is_empty(),
        "{} differ:\n{}",
        differences.len(),
        differences.join("\n")
    );
    Ok(())
}
