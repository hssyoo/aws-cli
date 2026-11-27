//! The `zec2` fake rows of `tests/diff/corpus/fakes.jsonl` through the
//! façade: each row's request body as the ec2 protocol serializes the
//! parameters the CLI builds from its arguments, and each row's canned
//! reply as the protocol parses it, against what the Python route sent and
//! printed for the row. Admission refuses `ec2`, so `cargo xtask diff`
//! compares these rows on the Python route alone; the expected bodies and
//! outputs below are that route's captures (`cargo xtask diff --a
//! candidate:python --b candidate:python --filter zec2`), with the random
//! idempotency tokens of the two rows that pass none given as parameters.
use super::{Result, ec2, python_eq};
use client::error::ParseError;
use client::http::{Headers, HttpResponse};
use client::testing::facade::{
    Model, botocore_timestamp, model_from_json, parse_case, parse_error_case, parse_options,
    serialize_case,
};
use model::value::Value;
use serde_json::Value as JsonDoc;
use std::path::PathBuf;
use std::sync::Arc;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn zec2() -> Result<Arc<Model>> {
    let path = repo().join("tests/models/zec2/2020-01-01/service-2.json");
    Ok(model_from_json(&std::fs::read(path)?)?)
}

/// The corpus row `id`.
fn row(id: &str) -> Result<JsonDoc> {
    let corpus = std::fs::read_to_string(repo().join("tests/diff/corpus/fakes.jsonl"))?;
    for line in corpus.lines() {
        let row: JsonDoc = serde_json::from_str(line)?;
        if row["id"] == id {
            return Ok(row);
        }
    }
    Err(format!("no row {id}").into())
}

/// The row's first canned reply.
fn reply(row: &JsonDoc) -> Result<HttpResponse> {
    let reply = &row["responses"][0];
    let headers = reply["headers"].as_array().ok_or("headers")?;
    let pairs = headers
        .iter()
        .filter_map(|pair| Some((pair[0].as_str()?, pair[1].as_str()?)));
    Ok(HttpResponse {
        status: u16::try_from(reply["status"].as_u64().unwrap_or(200))?,
        headers: Headers::from_pairs(pairs),
        body: reply["body"].as_str().ok_or("body")?.as_bytes().to_vec(),
    })
}

/// The CLI's parse hooks at its defaults: a timestamp as `isoformat()` in
/// UTC (`cli_timestamp_format = iso8601`), a blob's text as it is.
fn cli_timestamp(value: &Value) -> std::result::Result<Value, ParseError> {
    match botocore_timestamp(value)? {
        Value::Timestamp(at) => {
            let zoned = at.zoned(0).ok_or_else(|| ParseError::new("zone"))?;
            Ok(Value::String(zoned.isoformat()))
        }
        other => Ok(other),
    }
}

fn cli_blob(value: &Value) -> std::result::Result<Value, ParseError> {
    Ok(value.clone())
}

fn serialized(model: &Model, params: Value) -> Result<String> {
    let request = serialize_case(ec2()?, model, "RunGadgets", &params)?;
    assert_eq!(
        request.headers.get("Content-Type"),
        Some("application/x-www-form-urlencoded; charset=utf-8")
    );
    assert_eq!((request.method.as_str(), &*request.path), ("POST", "/"));
    Ok(String::from_utf8(request.body)?)
}

/// The output the CLI prints for the row's reply: the parsed output
/// without `ResponseMetadata`.
fn printed(model: &Model, row: &JsonDoc) -> Result<Value> {
    let options = parse_options(&cli_timestamp, &cli_blob);
    let Value::Map(mut parsed) = parse_case(ec2()?, model, "RunGadgets", &reply(row)?, &options)?
    else {
        return Err("not a structure".into());
    };
    parsed.remove("ResponseMetadata");
    Ok(Value::Map(parsed))
}

fn json(text: &str) -> Result<Value> {
    Ok(Value::from_json(text.as_bytes())?)
}

/// `fakes-zec2-run-gadgets`: every member type and naming rule.
#[test]
fn the_full_row_matches_the_python_route() -> Result<()> {
    let model = zec2()?;
    let row = row("fakes-zec2-run-gadgets")?;
    let Value::Map(mut params) = json(
        r#"{"GadgetName": "g a&b", "Plain": "p", "DryRun": true, "Active": false, "Count": 1,
            "Bare": 2, "Total": 3, "Wide": 9007199254740993, "Ratio": 1.5, "Exact": 0.1,
            "Data": null, "Raw": null,
            "Created": "2015-08-30T12:36:00Z", "Stamp": "2015-08-30T12:36:00Z",
            "Ids": ["i1", "i2"], "Names": ["n1"], "Loose": ["l1"], "Sizes": [1, 2],
            "Totals": [5], "Labels": ["a"],
            "Tags": [{"Key": "k", "Value": "v"}, {"Key": "k2", "Value": ""}],
            "Placement": {"Zone": "z", "Plain": "p"}, "Spot": {"Zone": "y"},
            "Token": "t-1"}"#,
    )?
    else {
        return Err("params".into());
    };
    // `--data AAFi`: the CLI's base64 blob argument, decoded.
    for blob in ["Data", "Raw"] {
        params.insert(blob.into(), Value::Blob(vec![0, 1, b'b']));
    }
    assert_eq!(
        serialized(&model, Value::Map(params))?,
        "Action=RunGadgets&Version=2020-01-01&GadgetName=g+a%26b&Plain=p&DryRun=true\
         &Active=false&Count=1&Bare=2&Total=3&Wide=9007199254740993&Ratio=1.5&Exact=0.1\
         &Data=AAFi&Raw=AAFi&Created=2015-08-30T12%3A36%3A00Z&Stamp=2015-08-30T12%3A36%3A00Z\
         &GadgetId.1=i1&GadgetId.2=i2&NameSet.1=n1&Loose.1=l1&SizeSet.1=1&SizeSet.2=2\
         &TotalSet.1=5&LabelSet.1=a&Tag.1.Key=k&Tag.1.Value=v&Tag.2.Key=k2&Tag.2.Value=\
         &Placement.AvailabilityZone=z&Placement.Plain=p&Spot.AvailabilityZone=y\
         &ClientToken=t-1"
    );
    let expected = json(
        r#"{"GadgetName": "g", "Plain": "p", "DryRun": true, "Active": false, "Count": 1,
            "Bare": 2, "Total": 3, "Wide": 9007199254740993, "Ratio": 1.5, "Exact": 0.1,
            "Data": "AAFi", "Raw": "AAFi", "Created": "2015-08-30T12:36:00+00:00",
            "Stamp": "2015-08-30T12:36:00+00:00", "Ids": ["i1", "i2"], "Names": ["n1"],
            "Loose": ["l1"], "Sizes": [1, 2], "Totals": [5], "Labels": ["a"],
            "Tags": [{"Key": "k", "Value": "v"}], "Placement": {"Zone": "z", "Plain": "p"},
            "Spot": {"Zone": "y"}, "Fraction": 0.25, "Part": 2.5, "RequestId": "r-1"}"#,
    )?;
    let parsed = printed(&model, &row)?;
    assert!(
        python_eq(&parsed, &expected),
        "parsed {parsed:?}\nexpected {expected:?}"
    );
    // The CLI prints the members in model order, as the parser builds them.
    let (Value::Map(parsed), Value::Map(expected)) = (&parsed, &expected) else {
        return Err("maps".into());
    };
    let keys = |map: &model::value::Map| map.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
    assert_eq!(keys(parsed), keys(expected));
    Ok(())
}

/// `fakes-zec2-run-gadgets-minimal`: only the token, and a reply holding
/// only the request id.
#[test]
fn the_minimal_row_matches_the_python_route() -> Result<()> {
    let model = zec2()?;
    let row = row("fakes-zec2-run-gadgets-minimal")?;
    let params = json(r#"{"Token": "78e32d37-0b6f-4242-9f41-43da6cf619da"}"#)?;
    assert_eq!(
        serialized(&model, params)?,
        "Action=RunGadgets&Version=2020-01-01&ClientToken=78e32d37-0b6f-4242-9f41-43da6cf619da"
    );
    let parsed = printed(&model, &row)?;
    assert!(
        python_eq(&parsed, &json(r#"{"RequestId": "r-2"}"#)?),
        "{parsed:?}"
    );
    Ok(())
}

/// `fakes-zec2-run-gadgets-unmodeled-error`: the CLI prints `An error
/// occurred (InvalidGadget.Malformed) when calling the RunGadgets
/// operation: bad gadget`, from the envelope's code and message.
#[test]
fn the_error_row_matches_the_python_route() -> Result<()> {
    let model = zec2()?;
    let row = row("fakes-zec2-run-gadgets-unmodeled-error")?;
    let params = json(r#"{"GadgetName": "g", "Token": "8feb964c-0fa2-4d5f-b57e-9e4bbccde90c"}"#)?;
    assert_eq!(
        serialized(&model, params)?,
        "Action=RunGadgets&Version=2020-01-01&GadgetName=g\
         &ClientToken=8feb964c-0fa2-4d5f-b57e-9e4bbccde90c"
    );
    let options = parse_options(&cli_timestamp, &cli_blob);
    let error = parse_error_case(ec2()?, &model, "RunGadgets", &reply(&row)?, &options)?;
    assert_eq!(error.status, 400);
    let text = |key: &str| error.error.get(key).and_then(Value::as_str);
    assert_eq!(text("Code"), Some("InvalidGadget.Malformed"));
    assert_eq!(text("Message"), Some("bad gadget"));
    assert_eq!(error.request_id.as_deref(), Some("r-3"));
    assert!(error.members.is_empty(), "{:?}", error.members);
    Ok(())
}
