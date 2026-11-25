//! The differential test: awscli's `JSONFormatter`, `TextFormatter` and
//! `iso_format` in the harness venv's Python against this crate, over the
//! corpus below, and `TextFormatter` over paginated responses. Python
//! produces every expected byte at test time (`oracle.py`); nothing
//! expected is committed.
use super::*;
use model::value::Map;
use std::path::PathBuf;
use timestamp::Timestamp;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Responses as JSON, `{"$bytes": base64}` for bytes and
/// `{"$datetime": microseconds}` for an aware UTC datetime.
const RESPONSES: &str = r#"[
  {},
  {"ResponseMetadata": {"RequestId": "r"}},
  {"ResponseMetadata": {"RequestId": "r"}, "Id": "x"},
  {"Name": "flat", "Count": 3, "On": true, "Off": false, "None": null},
  {"Text": "t", "Doc": [NaN, Infinity, -Infinity], "F": NaN},
  {"Outer": {"Inner": {"Deep": "v", "N": 1}, "Leaf": "l"}, "Top": "t"},
  {"Tags": ["a", "b", "c"], "Empty": [], "EmptyMap": {}},
  {"Items": [{"Name": "a", "Size": 1}, {"Name": "b", "Extra": [1, 2], "Map": {"k": "v"}}]},
  {"Grid": [["a", "b"], "c", [["d"]]]},
  {"Map": {"z": 1, "a": {"x": [true, null]}}},
  {"Big": 123456789012345678901234567890, "Neg": -12345678901234567890},
  {"Floats": [1.5, 1e-07, 1e+16, 0.1, -0.0, 3.0, 123456789.123456789]},
  {"Time": {"$datetime": 1440938160500000}, "Whole": {"$datetime": 0}},
  {"Blob": {"$bytes": "AAEC/w=="}, "Text": "plain"},
  {"Wide": "中文字符", "Combining": "é ä", "Emoji": "😀 🇬🇧", "Rtl": "שלום"},
  {"Control": "bell\u0007 tab\t nl\n cr\r nul\u0000 del\u007f"},
  {"Quote": "it's \"q\" back\\slash"},
  {"Long": "LONG"},
  {"Mixed": [{"k": 1}, "s"]},
  {"List": [[1, [2, [3]]], {"a": 1}]},
  ["bare", 1, null],
  "scalar"
]"#;

/// Paginated responses: a `PageIterator`'s pages, its result keys and
/// non-aggregate keys as the paginators file gives them, its resume token,
/// and the failure that ends its pages. The dotted keys are athena's
/// GetQueryResults, devops-guru's GetResourceCollection and ecr's
/// DescribeImageScanFindings.
const PAGES: &str = r#"[
  {"result_keys": ["Things"], "non_aggregate_keys": [], "resume_token": null, "fail": null,
   "pages": [{"Things": ["a", "b"], "NextToken": "t1"}, {"Things": ["c"]}]},
  {"result_keys": ["Things"], "non_aggregate_keys": [],
   "resume_token": "eyJOZXh0VG9rZW4iOiAidDIifQ==", "fail": null,
   "pages": [{"Things": [{"Name": "a", "Size": 1}, {"Name": "b", "Tags": ["x"]}],
              "NextToken": "t1"},
             {"Things": []}]},
  {"result_keys": ["Things"], "non_aggregate_keys": [], "resume_token": "", "fail": null,
   "pages": [{"Things": ["a"]}]},
  {"result_keys": ["ResultSet.Rows"],
   "non_aggregate_keys": ["ResultSet.ResultSetMetadata", "UpdateCount"],
   "resume_token": null, "fail": null,
   "pages": [{"ResultSet": {"Rows": [{"Data": [{"VarCharValue": "x"}]}],
                            "ResultSetMetadata": {"ColumnInfo": [{"Name": "c", "Type": "t"}]}},
              "UpdateCount": 0, "NextToken": "n"},
             {"ResultSet": {"Rows": [{"Data": [{"VarCharValue": "y"}]}],
                            "ResultSetMetadata": {"ColumnInfo": [{"Name": "c", "Type": "t"}]}},
              "UpdateCount": 0}]},
  {"result_keys": ["ResourceCollection.CloudFormation.StackNames", "ResourceCollection.Tags"],
   "non_aggregate_keys": ["ResourceCollection"], "resume_token": null, "fail": null,
   "pages": [{"ResourceCollection": {"Tags": [{"AppBoundaryKey": "k", "TagValues": ["v"]}]},
              "NextToken": "n"},
             {"ResourceCollection": {"CloudFormation": {"StackNames": ["s1", "s2"]}}}]},
  {"result_keys": ["ResourceCollection.CloudFormation.StackNames", "ResourceCollection.Tags"],
   "non_aggregate_keys": ["ResourceCollection"], "resume_token": null, "fail": null,
   "pages": [{"NextToken": "n"}, {"ResourceCollection": {}}]},
  {"result_keys": ["imageScanFindings.findings", "imageScanFindings.enhancedFindings"],
   "non_aggregate_keys": ["registryId", "imageScanFindings"], "resume_token": null, "fail": null,
   "pages": [{"registryId": "r"}]},
  {"result_keys": ["imageScanFindings.findings", "imageScanFindings.enhancedFindings"],
   "non_aggregate_keys": ["registryId", "imageScanFindings"], "resume_token": null, "fail": null,
   "pages": [{"registryId": "r", "imageScanFindings": {"findings": [{"name": "f"}],
              "imageScanCompletedAt": {"$datetime": 0}}},
             {"imageScanFindings": {"enhancedFindings": [{"type": "e"}]}}]},
  {"result_keys": ["Alarms", "Composite"], "non_aggregate_keys": [], "resume_token": null,
   "fail": null,
   "pages": [{"Alarms": [{"N": 1}]}, {"Composite": [{"N": 2}]}, {}]},
  {"result_keys": ["Output", "Count"], "non_aggregate_keys": [], "resume_token": "tok",
   "fail": null,
   "pages": [{"Output": "abc", "Count": 3}, {"Output": "d\u00e9f \u4e2d", "Count": 4}]},
  {"result_keys": ["Things"], "non_aggregate_keys": [], "resume_token": "tok",
   "fail": "An error occurred (Boom) when calling the ListThings operation: no",
   "pages": [{"Things": ["a"]}, {"Things": ["b"]}]},
  {"result_keys": ["Things"], "non_aggregate_keys": [], "resume_token": null, "fail": "first",
   "pages": []},
  {"result_keys": ["Mixed"], "non_aggregate_keys": [], "resume_token": null, "fail": null,
   "pages": [{"Mixed": ["s"]}, {"Mixed": [{"k": 1}, "s"]}, {"Mixed": ["t"]}]},
  {"result_keys": ["A.B.C"], "non_aggregate_keys": ["A"], "resume_token": null, "fail": null,
   "pages": [{"A": "xBy"}]},
  {"result_keys": ["A.B.C"], "non_aggregate_keys": ["A"], "resume_token": null, "fail": null,
   "pages": [{"A": "xyz"}]},
  {"result_keys": ["A.B"], "non_aggregate_keys": ["A"], "resume_token": null, "fail": null,
   "pages": [{"A": [1]}]},
  {"result_keys": ["A.B.C"], "non_aggregate_keys": ["A"], "resume_token": null, "fail": null,
   "pages": [{"A": {"$bytes": "AAE="}}]},
  {"result_keys": ["A.B"], "non_aggregate_keys": ["A"], "resume_token": null, "fail": null,
   "pages": [{"A": {"$bytes": "AAE="}}]},
  {"result_keys": ["A.B.C"], "non_aggregate_keys": ["A"], "resume_token": null, "fail": null,
   "pages": [{"A": {"$datetime": 0}}]},
  {"result_keys": ["A.B"], "non_aggregate_keys": ["A"], "resume_token": null, "fail": null,
   "pages": [{"A": 5}]},
  {"result_keys": ["A.B.C"], "non_aggregate_keys": ["A"], "resume_token": null, "fail": null,
   "pages": [{"A": true}]}
]"#;

/// Wire timestamps as the query (text) and json (numbers, text) protocols
/// deliver them.
const TIMESTAMPS: &str = r#"[
  "2015-08-30T12:36:00Z", "2015-08-30T12:36:00.123Z", "2015-08-30T12:36:00.123456789Z",
  "2015-08-30T13:36:00+01:00", "2015-08-30T11:06:00-0130", "2015-08-30T12:36:00",
  "Sun, 30 Aug 2015 12:36:00 GMT", "30 Aug 2015 13:36:00 +0100",
  1440938160, 1440938160.5, 1440938160.123456, 0, -1.5, "1440938160", " 1440938160.25 "
]"#;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Runs `oracle.py` with `zone` as `TZ` in the harness venv `cargo xtask
/// parity` provisions and names in `PARITY_PYTHON`.
fn oracle(corpus: &str, zone: &str) -> Result<serde_json::Value> {
    let python = std::env::var_os("PARITY_PYTHON")
        .ok_or("PARITY_PYTHON is unset: these tests run under `cargo xtask parity`")?;
    let mut child = std::process::Command::new(python)
        .arg("crates/format/src/oracle.py")
        .current_dir(repository())
        .env_clear()
        .envs([
            ("PATH", "/usr/bin:/bin"),
            ("LC_ALL", "C.UTF-8"),
            ("PYTHONHASHSEED", "0"),
        ])
        .env("TZ", zone)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    std::io::Write::write_all(&mut child.stdin.take().ok_or("stdin")?, corpus.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// The corpus Python and Rust both read: the long string expanded here.
fn corpus() -> String {
    let long = "x".repeat(100_000) + "é";
    let responses = RESPONSES.replace("\"LONG\"", &format!("\"{long}\""));
    format!(r#"{{"responses": {responses}, "timestamps": {TIMESTAMPS}, "pages": {PAGES}}}"#)
}

/// `oracle.py`'s `decode`.
fn decode(value: Value) -> Value {
    match value {
        Value::Map(map) => tagged(&map).unwrap_or_else(|| {
            Value::Map(
                map.into_iter()
                    .map(|(k, v)| (k, decode(v)))
                    .collect::<Map>(),
            )
        }),
        Value::List(items) => Value::List(items.into_iter().map(decode).collect()),
        other => other,
    }
}

fn tagged(map: &Map) -> Option<Value> {
    if map.len() != 1 {
        return None;
    }
    let (key, value) = map.iter().next()?;
    match (key.as_str()?, value) {
        ("$bytes", Value::String(text)) => pytext::base64::decode(text).ok().map(Value::Blob),
        ("$datetime", Value::Integer(micros)) => {
            Timestamp::from_unix_micros(*micros).map(Value::Timestamp)
        }
        _ => None,
    }
}

fn rust_row(response: &Value) -> serde_json::Value {
    let cell = |format| match render(response, format) {
        Ok(text) => serde_json::json!({ "ok": text }),
        Err(partial) => serde_json::json!({ "error": partial.error.message }),
    };
    serde_json::json!([cell(OutputFormat::Json), cell(OutputFormat::Text)])
}

#[test]
#[ignore = "runs under cargo xtask parity"]
fn json_and_text_match_awscli_s_formatters() -> Result<()> {
    let corpus = corpus();
    let expected = oracle(&corpus, "UTC")?;
    // `json.loads`, as the oracle reads it: `NaN` and the infinities included.
    let parsed = model::pyjson::loads(&corpus).map_err(|error| format!("{error:?}"))?;
    let Value::Map(parsed) = parsed else {
        return Err("corpus".into());
    };
    let Some(Value::List(responses)) = parsed.get("responses") else {
        return Err("responses".into());
    };
    let expected = expected["responses"]
        .as_array()
        .ok_or("expected responses")?;
    assert_eq!(responses.len(), expected.len());
    let mut differences = Vec::new();
    for (index, (response, wanted)) in responses.iter().zip(expected).enumerate() {
        let actual = rust_row(&decode(response.clone()));
        if &actual != wanted {
            differences.push(format!("response {index}: rust {actual} python {wanted}"));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
    Ok(())
}

/// The listed pages of one corpus case.
fn listed(case: &Value) -> Option<crate::pages::tests::Listed> {
    let Value::Map(case) = case else {
        return None;
    };
    let expressions = |name: &str| match case.get(name) {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(text) => model::paginators::Expression::parse(text),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let text = |name: &str| match case.get(name) {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    };
    let Some(Value::List(pages)) = case.get("pages") else {
        return None;
    };
    let mut listed =
        crate::pages::tests::Listed::new(pages.iter().cloned().map(decode).collect(), &[]);
    listed.result_keys = expressions("result_keys");
    listed.non_aggregate_keys = expressions("non_aggregate_keys");
    listed.resume_token = text("resume_token");
    listed.fail = text("fail");
    Some(listed)
}

fn rust_pages(mut listed: crate::pages::tests::Listed) -> serde_json::Value {
    let mut out = String::new();
    let result = crate::pages::text(&mut listed, &mut |text| {
        out.push_str(text);
        Ok(())
    });
    let error = match result {
        Ok(()) => return serde_json::json!({ "ok": out }),
        Err(crate::pages::Stopped::Page(error)) => error,
        Err(crate::pages::Stopped::Format(error)) => error.message,
        Err(crate::pages::Stopped::Write(error)) => error.to_string(),
    };
    serde_json::json!({ "error": error, "written": out })
}

/// `TextFormatter` over each paginated case: the text written page by
/// page, and on a failure what was written before it and the failure.
#[test]
#[ignore = "runs under cargo xtask parity"]
fn paginated_text_matches_awscli_s_text_formatter() -> Result<()> {
    let expected = oracle(&corpus(), "UTC")?;
    let expected = expected["pages"].as_array().ok_or("expected pages")?;
    let Value::List(cases) = Value::from_json(PAGES.as_bytes())? else {
        return Err("pages".into());
    };
    assert_eq!(cases.len(), expected.len());
    let mut differences = Vec::new();
    for (index, (case, wanted)) in cases.iter().zip(expected).enumerate() {
        let actual = rust_pages(listed(case).ok_or("case")?);
        if &actual != wanted {
            differences.push(format!("case {index}: rust {actual} python {wanted}"));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
    Ok(())
}

/// `iso_format` under a zone without daylight-saving time, at UTC and at
/// +05:30, and each result printed through both formatters.
#[test]
#[ignore = "runs under cargo xtask parity"]
fn iso8601_timestamps_match_iso_format() -> Result<()> {
    for (zone, offset) in [("UTC", 0), ("IST-05:30", 19_800)] {
        let expected = oracle(&corpus(), zone)?;
        let expected = expected["timestamps"]
            .as_array()
            .ok_or("expected timestamps")?;
        let Value::List(wires) = Value::from_json(TIMESTAMPS.as_bytes())? else {
            return Err("timestamps".into());
        };
        let parser = TimestampParser::new(TimestampFormat::Iso8601, Some(offset));
        for (wire, wanted) in wires.iter().zip(expected) {
            let actual = match parser.parse(wire) {
                Ok(Value::String(text)) => serde_json::json!({ "ok": text }),
                other => serde_json::json!({ "rust": format!("{other:?}") }),
            };
            assert_eq!(&actual, wanted, "{zone} {wire:?}");
        }
    }
    Ok(())
}

/// A response nested to `json.loads`' limit renders as JSON and as text,
/// and drops, on a thread with the default stack: the formatters recurse
/// no deeper than the reader holds.
#[test]
fn a_response_at_the_reader_s_limit_renders_on_a_default_thread() -> Result<()> {
    let rendered = std::thread::spawn(|| {
        let mut doc = Value::List(Vec::new());
        for _ in 1..model::pyjson::MAX_DEPTH - 1 {
            doc = Value::List(vec![doc]);
        }
        let mut response = Map::new();
        response.insert("Doc".into(), doc);
        let response = Value::Map(response);
        let json = render(&response, OutputFormat::Json).map(|text| text.len());
        let text = render(&response, OutputFormat::Text).map(|text| text.len());
        drop(response);
        (json.is_ok(), text.is_ok())
    })
    .join()
    .map_err(|_| "rendering overflowed its stack")?;
    assert_eq!(rendered, (true, true));
    Ok(())
}
