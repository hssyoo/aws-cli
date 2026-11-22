//! The expected reports in `expected.json` are botocore's own, produced by
//! running `oracle.py` (beside this file) in the blackbox venv:
//!
//! ```text
//! target/blackbox/repo/venv/bin/python crates/model/src/validate/oracle.py \
//!     crates/model/src/validate/fixture.json > crates/model/src/validate/expected.json
//! ```
//!
//! Each case below builds the same parameters as the oracle's entry of the
//! same name.
use super::*;
use crate::shapes::Model;
use crate::testing::Result;
use crate::value::{Map, Value};
use timestamp::Timestamp;

fn model() -> Result<Model> {
    Ok(Model::from_json(include_bytes!("fixture.json"))?)
}

fn json(text: &str) -> Value {
    Value::from_json(text.as_bytes()).unwrap_or(Value::Null)
}

/// The oracle's `ok` parameters with `member` set to `value`.
fn ok_with(member: &str, value: Value) -> Value {
    let mut map = Map::new();
    map.insert("Req".into(), Value::String("abc".into()));
    map.insert("Nested".into(), json(r#"{"X":"x"}"#));
    map.insert(member.into(), value);
    Value::Map(map)
}

fn utc_day() -> Value {
    Value::Timestamp(Timestamp::from_unix(1_577_923_200, 0).unwrap_or(Timestamp::MIN))
}

fn json_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("missing", json("{}")),
        (
            "unknown",
            json(r#"{"Req":"abc","Nested":{"X":"a"},"Bogus":1,"Other":2}"#),
        ),
        (
            "types",
            json(
                r#"{"Req":5,"Nested":[],"Num":"1","Flag":"true","Opt":null,"List":"a",
                "Map":[],"Dbl":"x","Long":1.5}"#,
            ),
        ),
        (
            "ranges",
            json(
                r#"{"Req":"ab","Nested":{"X":"x"},"Num":0,"Long":9,"Dbl":0.25,"List":[],
                "Fmin":1}"#,
            ),
        ),
        ("nested", json(r#"{"Req":"abc","Nested":{"Z":1,"Y":"y"}}"#)),
        ("not_a_dict", json("[]")),
        (
            "everything",
            json(r#"{"Bogus":1,"Num":0,"Req":1,"List":[1],"Union":{}}"#),
        ),
    ]
}

fn member_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("map", ok_with("Map", json(r#"{"k":1,"kk":"x","kkk":0}"#))),
        ("list", ok_with("List", json(r#"["a",5,["x"],"abc"]"#))),
        ("union_empty", ok_with("Union", json("{}"))),
        ("union_two", ok_with("Union", json(r#"{"A":"abc","B":1}"#))),
        ("jsonvalue_ok", ok_with("Json", json(r#"{"a":[1,null]}"#))),
        ("blob", ok_with("Blob", Value::Integer(5))),
        ("blob_ok", ok_with("Blob", Value::String("text".into()))),
        ("timestamps_bad", ok_with("When", json("[1]"))),
        ("timestamp_none", ok_with("When", Value::Null)),
        ("timestamp_nan", ok_with("When", Value::Float(f64::NAN))),
        ("timestamps_ok", ok_with("When", Value::String("12".into()))),
        ("host_label", ok_with("Host", Value::String(String::new()))),
        (
            "repr",
            ok_with("Opt", json(r#"{"it's":["a\nb","q\"'",2.5e-7]}"#)),
        ),
        (
            "big_negative",
            ok_with("Num", json("-99999999999999999999")),
        ),
        ("big_positive", ok_with("Num", json("99999999999999999999"))),
        ("float_for_int", ok_with("Num", Value::Float(1.0))),
    ]
}

fn python_only_cases() -> Vec<(&'static str, Value)> {
    let mut bools = ok_with("Num", Value::Bool(true));
    if let Value::Map(map) = &mut bools {
        map.insert("Dbl".into(), Value::Bool(false));
        map.insert("Long".into(), Value::Bool(true));
    }
    let mut non_ascii = ok_with("Req", Value::String("é😀".into()));
    if let Value::Map(map) = &mut non_ascii {
        map.insert(
            "Opt".into(),
            Value::List(vec![Value::String("é😀\0".into())]),
        );
    }
    let mut document = Map::new();
    let mut inner = Map::new();
    inner.insert("b".into(), Value::Blob(b"x".to_vec()));
    document.insert(
        "a".into(),
        Value::List(vec![Value::Integer(1), Value::Map(inner)]),
    );
    document.insert("c".into(), utc_day());
    document.insert("d".into(), Value::Null);
    document.insert("e".into(), Value::Float(1.5));
    let mut blob_header = Map::new();
    blob_header.insert("a".into(), Value::Blob(b"x".to_vec()));
    vec![
        ("bools_are_ints", bools),
        ("non_ascii", non_ascii),
        ("document", ok_with("Doc", Value::Map(document))),
        ("document_top", ok_with("Doc", Value::Blob(b"raw".to_vec()))),
        ("jsonvalue", ok_with("Json", Value::Map(blob_header))),
        (
            "jsonvalue_datetime",
            ok_with("Json", Value::List(vec![utc_day()])),
        ),
    ]
}

#[test]
fn reports_match_botocore_for_every_error_kind() -> Result<()> {
    let model = model()?;
    let input = model
        .operation("Check")
        .and_then(|op| op.input)
        .ok_or("input")?;
    let expected: serde_json::Value = serde_json::from_slice(include_bytes!("expected.json"))?;
    let expected = expected.as_object().ok_or("expected is not an object")?;
    let mut cases = json_cases();
    cases.extend(member_cases());
    cases.extend(python_only_cases());
    assert_eq!(cases.len(), expected.len());
    for (name, params) in cases {
        let report = match validate(&model, input, &params) {
            Validation::Valid => None,
            Validation::Invalid(report) => Some(report.to_string()),
            Validation::Undecided(reason) => return Err(format!("{name}: {reason:?}").into()),
        };
        let wanted = expected.get(name).ok_or(name)?.as_str();
        assert_eq!(report.as_deref(), wanted, "{name}");
    }
    Ok(())
}

#[test]
fn the_error_message_wraps_the_report_and_no_input_shape_is_valid() -> Result<()> {
    let model = model()?;
    let check = model.operation("Check").ok_or("Check")?;
    let Validation::Invalid(report) = validate_input(&model, check, &json("{}")) else {
        return Err("{} validated".into());
    };
    let text = "Parameter validation failed:\nMissing required parameter in input: \"Req\"\n\
        Missing required parameter in input: \"Nested\"";
    assert_eq!(report.message(), text);
    let bare = model.operation("Bare").ok_or("Bare")?;
    assert!(matches!(
        validate_input(&model, bare, &json("[1]")),
        Validation::Valid
    ));
    Ok(())
}

#[test]
fn timestamps_python_accepts_are_valid() -> Result<()> {
    let model = model()?;
    let input = model
        .operation("Check")
        .and_then(|op| op.input)
        .ok_or("input")?;
    for value in [
        utc_day(),
        Value::String("2020-01-02T03:04:05Z".into()),
        Value::String("Thu, 02 Jan 2020 03:04:05 GMT".into()),
        Value::String(" 1_000.5 ".into()),
        Value::String("-12e3".into()),
        Value::Float(1.5),
        Value::Integer(-86_400),
        Value::Bool(true),
        Value::Blob(b"12".to_vec()),
    ] {
        let params = ok_with("When", value.clone());
        assert!(
            matches!(validate(&model, input, &params), Validation::Valid),
            "{value:?}"
        );
    }
    Ok(())
}

/// Where botocore's answer depends on dateutil's parser or on an exception
/// it does not catch, the validator declines to decide.
#[test]
fn what_only_python_can_decide_is_undecided() -> Result<()> {
    let model = model()?;
    let input = model
        .operation("Check")
        .and_then(|op| op.input)
        .ok_or("input")?;
    for (member, value) in [
        ("When", Value::String("next tuesday".into())),
        ("When", Value::String("inf".into())),
        ("When", Value::Float(1e300)),
        ("When", Value::Blob(b"Jan 5".to_vec())),
        ("Big", Value::Integer(1)),
    ] {
        let params = ok_with(member, value.clone());
        let outcome = validate(&model, input, &params);
        assert!(
            matches!(outcome, Validation::Undecided(_)),
            "{value:?}: {outcome:?}"
        );
    }
    Ok(())
}
