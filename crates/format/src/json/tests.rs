use super::*;
use model::value::{Map, Value};
use timestamp::Timestamp;

fn value(text: &str) -> Value {
    Value::from_json(text.as_bytes()).unwrap_or(Value::Null)
}

#[test]
fn json_is_json_dumps_indented_by_four_with_a_newline() {
    let text = json(&value(r#"{"b":[1,{"c":null}],"a":"é\u0001"}"#));
    assert_eq!(text, "{\n    \"b\": [\n        1,\n        {\n            \"c\": null\n        }\n    ],\n    \"a\": \"é\\u0001\"\n}\n");
    assert_eq!(json(&value("{}")), "");
    assert_eq!(json(&value("[]")), "[]\n");
}

/// `json_encoder`: a datetime prints its `isoformat`, bytes their base64.
#[test]
fn the_cli_encoder_handles_datetimes_and_bytes() {
    let mut map = Map::new();
    let at = Timestamp::from_unix_micros(1_440_938_160_500_000).unwrap_or(Timestamp::MIN);
    map.insert("t".into(), Value::Timestamp(at));
    map.insert("b".into(), Value::Blob(b"foo".to_vec()));
    let expected = "{\n    \"t\": \"2015-08-30T12:36:00.500000+00:00\",\n    \"b\": \"Zm9v\"\n}\n";
    assert_eq!(json(&Value::Map(map)), expected);
}
