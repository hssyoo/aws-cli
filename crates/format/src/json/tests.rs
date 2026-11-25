use super::*;
use model::value::{Map, Value};
use timestamp::Timestamp;

fn value(text: &str) -> Value {
    Value::from_json(text.as_bytes()).unwrap_or(Value::Null)
}

/// What `json` writes to stdout for `value`, as text.
fn printed(value: &Value) -> String {
    let mut out = Encoded::new(pytext::codec::Errors::SurrogateEscape);
    match json(value, &mut out) {
        Ok(()) => String::from_utf8_lossy(&out.into_bytes()).into_owned(),
        Err(error) => error.message,
    }
}

#[test]
fn json_is_json_dumps_indented_by_four_with_a_newline() {
    let text = printed(&value(r#"{"b":[1,{"c":null}],"a":"é\u0001"}"#));
    assert_eq!(
        text,
        concat!(
            "{\n    \"b\": [\n        1,\n        {\n            \"c\": null\n",
            "        }\n    ],\n    \"a\": \"é\\u0001\"\n}\n"
        )
    );
    assert_eq!(printed(&value("{}")), "");
    assert_eq!(printed(&value("[]")), "[]\n");
}

/// `json_encoder`: a datetime prints its `isoformat`, bytes their base64.
#[test]
fn the_cli_encoder_handles_datetimes_and_bytes() {
    let mut map = Map::new();
    let at = Timestamp::from_unix_micros(1_440_938_160_500_000).unwrap_or(Timestamp::MIN);
    map.insert("t".into(), Value::Timestamp(at));
    map.insert("b".into(), Value::Blob(b"foo".to_vec()));
    let expected = "{\n    \"t\": \"2015-08-30T12:36:00.500000+00:00\",\n    \"b\": \"Zm9v\"\n}\n";
    assert_eq!(printed(&Value::Map(map)), expected);
}

/// `_remove_request_id` on a response that is not a container raises.
#[test]
fn a_response_python_cannot_search_fails_as_the_formatter_does() {
    let error = crate::render(
        &Value::Null,
        crate::OutputFormat::Json,
        pytext::codec::Errors::SurrogateEscape,
    )
    .err();
    let message = error.map(|partial| partial.error.message);
    assert_eq!(
        message.as_deref(),
        Some("argument of type 'NoneType' is not a container or iterable")
    );
    assert_eq!(
        crate::render(
            &Value::Null,
            crate::OutputFormat::Off,
            pytext::codec::Errors::SurrogateEscape
        ),
        Ok(Vec::new())
    );
}
