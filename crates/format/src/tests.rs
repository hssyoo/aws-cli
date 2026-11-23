//! The differential test: awscli's `JSONFormatter`, `TextFormatter` and
//! `iso_format` in the blackbox venv's Python against this crate, over the
//! corpus below. Python produces every expected byte at test time
//! (`oracle.py`); nothing expected is committed.
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

/// Runs `oracle.py` in the blackbox venv with `zone` as `TZ`.
fn oracle(corpus: &str, zone: &str) -> Result<serde_json::Value> {
    let python = repository().join("target/blackbox/repo/venv/bin/python");
    if !python.exists() {
        return Err(format!(
            "{} is missing: provision the blackbox venv with \
             `cargo xtask blackbox --suite repo -- -k test_ls` once, then rerun",
            python.display()
        )
        .into());
    }
    let mut child = std::process::Command::new(python)
        .arg("crates/format/src/oracle.py")
        .current_dir(repository())
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
    format!(r#"{{"responses": {responses}, "timestamps": {TIMESTAMPS}}}"#)
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
    match map.iter().next()? {
        ("$bytes", Value::String(text)) => Some(Value::Blob(decode_base64(text))),
        ("$datetime", Value::Integer(micros)) => {
            Timestamp::from_unix_micros(*micros).map(Value::Timestamp)
        }
        _ => None,
    }
}

/// The test corpus's base64, decoded for the bytes values.
fn decode_base64(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let sextets: Vec<u32> = text
        .bytes()
        .filter_map(|byte| ALPHABET.iter().position(|a| *a == byte))
        .filter_map(|index| u32::try_from(index).ok())
        .collect();
    let mut bytes = Vec::new();
    for chunk in sextets.chunks(4) {
        let group = chunk
            .iter()
            .enumerate()
            .fold(0, |g, (i, s)| g | s << (18 - 6 * i));
        let count = chunk.len() * 6 / 8;
        bytes.extend(group.to_be_bytes()[1..=count].iter());
    }
    bytes
}

fn rust_row(response: &Value) -> serde_json::Value {
    let cell = |format| match render(response, format) {
        Ok(text) => serde_json::json!({ "ok": text }),
        Err(partial) => serde_json::json!({ "error": partial.error.message }),
    };
    serde_json::json!([cell(OutputFormat::Json), cell(OutputFormat::Text)])
}

#[test]
fn json_and_text_match_awscli_s_formatters() -> Result<()> {
    let corpus = corpus();
    let expected = oracle(&corpus, "UTC")?;
    let Value::Map(parsed) = Value::from_json(corpus.as_bytes())? else {
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

/// `iso_format` under a zone without daylight-saving time, at UTC and at
/// +05:30, and each result printed through both formatters.
#[test]
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
