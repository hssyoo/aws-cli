use super::*;
use crate::error::SerializeError;
use crate::testing::facade;
use model::schema::SchemaSet;
use serde_json::{Value as Json, json};
use std::path::{Path, PathBuf};

mod c2j;
mod json;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// `JSONSerializer`'s body value for `input`, through [`write_value`].
fn written(schema: Schema<'_>, input: &Value) -> std::result::Result<json::Writer, SerdeError> {
    let mut writer = json::Writer::default();
    write_value(schema, input, &mut writer)?;
    Ok(writer)
}

fn body(
    set: &SchemaSet,
    operation: &str,
    input: &Value,
) -> std::result::Result<Vec<u8>, SerializeError> {
    let schema = set.service().operation(operation).and_then(|op| op.input());
    let tree = match schema {
        Some(schema) => written(schema, input)?.finish()?,
        None => Value::Map(Map::new()),
    };
    let text = model::pyjson::dumps(&tree, &model::pyjson::Options::default(), None)
        .map_err(|error| SerializeError::new(error.to_string()))?;
    Ok(text.into_bytes())
}

fn parsed(
    schema: Schema<'_>,
    wire: &Value,
    options: &ParseOptions<'_>,
) -> std::result::Result<Value, ParseError> {
    build_value(schema, &mut json::Cursor(wire), options)
}

const IDENTITY: ParseOptions<'static> = ParseOptions {
    timestamp: &|value| Ok(value.clone()),
    blob: &|value| Ok(value.clone()),
};

/// A one-operation model: `Op` takes and returns `S`.
fn fixture() -> Result<SchemaSet> {
    c2j::schema_set(&json!({
        "operations": {"Op": {"input": {"shape": "S"}, "output": {"shape": "S"}}},
        "shapes": {
            "S": {"type": "structure", "members": {
                "A": {"shape": "Str"}, "B": {"shape": "Str", "locationName": "b"},
                "L": {"shape": "L"}, "M": {"shape": "M"}, "T": {"shape": "T"},
                "D": {"shape": "D"}, "U": {"shape": "U"}, "N": {"shape": "S"}}},
            "Str": {"type": "string"},
            "T": {"type": "timestamp"},
            "L": {"type": "list", "member": {"shape": "T"}},
            "M": {"type": "map", "key": {"shape": "Str"}, "value": {"shape": "Str"}},
            "D": {"type": "structure", "members": {}, "document": true},
            "U": {"type": "structure", "union": true, "members": {
                "X": {"shape": "Str"}, "Y": {"shape": "Str", "locationName": "y"}}}
        }
    }))
}

fn root(set: &SchemaSet) -> Result<Schema<'_>> {
    Ok(set.shape_named("S").ok_or("S")?)
}

fn value(json: &Json) -> Value {
    Value::from_serde(json)
}

#[test]
fn members_are_written_in_the_input_order_under_their_wire_names() -> Result<()> {
    let set = fixture()?;
    let input = value(&json!({"M": {"z": "1", "a": "2"}, "B": "b", "A": "a"}));
    let tree = written(root(&set)?, &input)?.finish()?;
    assert_eq!(
        tree,
        value(&json!({"M": {"z": "1", "a": "2"}, "b": "b", "A": "a"}))
    );
    Ok(())
}

#[test]
fn an_unknown_member_is_the_key_error_text() -> Result<()> {
    let set = fixture()?;
    let error = written(root(&set)?, &value(&json!({"A": "a", "Nope": 1}))).err();
    assert_eq!(error, Some(SerdeError::new("'Nope'")));
    Ok(())
}

#[test]
fn a_container_given_another_value_fails_as_botocore_does() -> Result<()> {
    let set = fixture()?;
    let cases = [
        (json!({"N": [1]}), "'list' object has no attribute 'items'"),
        (
            json!({"N": null}),
            "'NoneType' object has no attribute 'items'",
        ),
        (json!({"L": "x"}), "a list is required"),
        (json!({"M": ["x"]}), "a dict is required"),
    ];
    for (input, text) in cases {
        let error = written(root(&set)?, &value(&input)).err();
        assert_eq!(error, Some(SerdeError::new(text)), "{input}");
    }
    Ok(())
}

#[test]
fn null_list_items_and_map_values_are_the_codecs_write_null() -> Result<()> {
    let set = fixture()?;
    let input =
        value(&json!({"L": [null], "M": {"k": null}, "A": null, "D": {"x": [1]}, "U": {"X": "x"}}));
    let writer = written(root(&set)?, &input)?;
    let calls = [
        "write_struct",
        "write_list",
        "write_null",
        "write_map",
        "write_null",
        "write_string",
        "write_document",
        "write_struct",
        "write_string",
    ];
    assert_eq!(writer.calls, calls);
    let tree = writer.finish()?;
    let expected =
        json!({"L": [null], "M": {"k": null}, "A": null, "D": {"x": [1]}, "U": {"X": "x"}});
    assert_eq!(tree, value(&expected));
    Ok(())
}

#[test]
fn members_are_read_in_schema_order_and_null_members_dropped() -> Result<()> {
    let set = fixture()?;
    let wire =
        value(&json!({"M": {"z": null, "a": "1"}, "b": "x", "A": null, "L": [null, 5], "N": null}));
    // A timestamp hook that marks what it saw: a `null` item never reaches it.
    let marking = ParseOptions {
        timestamp: &|value| Ok(Value::List(vec![value.clone()])),
        ..IDENTITY
    };
    let parsed = parsed(root(&set)?, &wire, &marking)?;
    let expected = json!({"B": "x", "L": [null, [5]], "M": {"z": null, "a": "1"}});
    assert_eq!(parsed, value(&expected));
    let null = build_value(root(&set)?, &mut json::Cursor(&Value::Null), &IDENTITY)?;
    assert_eq!(null, Value::Null);
    Ok(())
}

#[test]
fn timestamps_and_blobs_go_through_the_hooks_whose_errors_pass_through() -> Result<()> {
    let set = fixture()?;
    let hooks = ParseOptions {
        timestamp: &|value| match value {
            Value::Integer(0) => Err(ParseError::new("hook refused 0")),
            other => Ok(Value::List(vec![other.clone()])),
        },
        blob: &|value| Ok(value.clone()),
    };
    let parsed = parsed(root(&set)?, &value(&json!({"T": 1, "L": [2]})), &hooks)?;
    assert_eq!(parsed, value(&json!({"L": [[2]], "T": [1]})));
    let error = self::parsed(root(&set)?, &value(&json!({"N": {"L": [0]}})), &hooks).err();
    assert_eq!(error, Some(ParseError::new("hook refused 0")));
    Ok(())
}

#[test]
fn a_union_with_one_unknown_member_is_sdk_unknown_member() -> Result<()> {
    let set = fixture()?;
    let schema = root(&set)?;
    let read = |wire: Json| self::parsed(schema, &value(&json!({"U": wire})), &IDENTITY);
    let unknown = json!({"U": {"SDK_UNKNOWN_MEMBER": {"name": "Z"}}});
    assert_eq!(
        read(json!({"Z": 1, "__type": "t", "X": null}))?,
        value(&unknown)
    );
    assert_eq!(read(json!({"y": "1"}))?, value(&json!({"U": {"Y": "1"}})));
    let text = "Invalid service response: U must have one and only one member set.";
    for wire in [json!({"X": "1", "y": "2"}), json!({"__type": "t"})] {
        assert_eq!(read(wire).err(), Some(ParseError::new(text)));
    }
    Ok(())
}

#[test]
fn serde_errors_keep_their_text_as_call_errors() {
    let error = SerdeError::new("a dict is required");
    assert_eq!(
        SerializeError::from(error.clone()).message,
        "a dict is required"
    );
    assert_eq!(
        ParseError::from(error.clone()).message,
        "a dict is required"
    );
    assert_eq!(
        SerdeError::from(ParseError::new("a dict is required")),
        error
    );
}

// ------------------------------------------------- parity layer A replay

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The case encoding `oracle.py` decodes: `{"$blob": base64}` and
/// `{"$datetime": ISO 8601}` (UTC in every case).
fn decoded(json: &Json) -> Result<Value> {
    Ok(match json {
        Json::Object(map) if map.len() == 1 && map.contains_key("$blob") => Value::Blob(
            pytext::base64::decode(map["$blob"].as_str().ok_or("blob")?)?,
        ),
        Json::Object(map) if map.len() == 1 && map.contains_key("$datetime") => {
            let text = map["$datetime"].as_str().ok_or("datetime")?;
            let zoned = timestamp::Timestamp::parse_iso8601_zoned(text).ok_or("datetime")?;
            Value::Timestamp(timestamp::Timestamp::from_civil(zoned.civil).ok_or("range")?)
        }
        Json::Object(map) => {
            let mut decoded = Map::new();
            for (key, item) in map {
                decoded.insert(key.clone(), self::decoded(item)?);
            }
            Value::Map(decoded)
        }
        Json::Array(items) => Value::List(items.iter().map(self::decoded).collect::<Result<_>>()?),
        other => Value::from_serde(other),
    })
}

/// The `service-2.json` a case's model is read from: the latest version.
fn model_file(case: &Json) -> Result<PathBuf> {
    let data = match case["models"].as_str() {
        Some("fake") => "tests/models",
        _ => "aws-cli/awscli/botocore/data",
    };
    let service = repository()
        .join(data)
        .join(case["service"].as_str().ok_or("service")?);
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&service)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<_>>()?;
    versions.sort();
    Ok(versions.pop().ok_or("no version")?.join("service-2.json"))
}

/// What one case replays: today's `json` serializer against the walk, and
/// for a 2xx response today's parser against the walk, both read from one
/// `service-2.json`.
fn replay(case: &Json, counts: &mut [usize; 2]) -> Result<()> {
    let bytes = std::fs::read(model_file(case)?)?;
    let old = facade::model_from_json(&bytes)?;
    if crate::protocol::for_model(&old).map(|p| p.name()) != Some("json") {
        return Ok(());
    }
    let set = c2j::schema_set(&serde_json::from_slice(&bytes)?)?;
    let operation = case["operation"].as_str().ok_or("operation")?;
    let input = decoded(&case["input"])?;
    let protocol = facade::protocol_named("json").ok_or("json")?;
    let expected =
        facade::serialize_case(protocol, &old, operation, &input).map(|request| request.body);
    assert_eq!(
        body(&set, operation, &input),
        expected,
        "{}: request body",
        case["name"]
    );
    counts[0] += 1;
    let Some(response) = case
        .get("response")
        .filter(|r| r["status"].as_u64().unwrap_or(200) < 300)
    else {
        return Ok(());
    };
    let response = crate::http::HttpResponse {
        status: 200,
        headers: crate::http::Headers::new(),
        body: response["body"]
            .as_str()
            .unwrap_or_default()
            .as_bytes()
            .to_vec(),
    };
    // An operation without output parses no body.
    let Some(output) = set
        .service()
        .operation(operation)
        .and_then(|op| op.output())
    else {
        return Ok(());
    };
    let wire = match std::str::from_utf8(&response.body)? {
        "" => Value::Map(Map::new()),
        text => model::pyjson::loads(text).map_err(|error| format!("{error:?}"))?,
    };
    for options in [ParseOptions::BOTOCORE, IDENTITY] {
        let mut expected = facade::parse_case(protocol, &old, operation, &response, &options);
        if let Ok(Value::Map(map)) = &mut expected {
            map.remove("ResponseMetadata");
        }
        // As text: a NaN is not equal to itself.
        let (actual, expected) = (parsed(output, &wire, &options), expected);
        assert_eq!(
            format!("{actual:?}"),
            format!("{expected:?}"),
            "{}: parsed output",
            case["name"]
        );
    }
    counts[1] += 1;
    Ok(())
}

/// Every `json`-protocol case of parity layer A, replayed through the test
/// codec: the walks give today's request bodies and parsed outputs.
#[test]
fn parity_layer_a_json_cases_replay_through_the_walks() -> Result<()> {
    let cases = repository().join("tests/parity/cases");
    let mut counts = [0, 0];
    let mut files: Vec<PathBuf> = std::fs::read_dir(&cases)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<_>>()?;
    files.sort();
    for file in files
        .iter()
        .filter(|file| file.extension() == Some("json".as_ref()))
    {
        let list: Vec<Json> = serde_json::from_slice(&std::fs::read(Path::new(file))?)?;
        for case in &list {
            replay(case, &mut counts)?;
        }
    }
    // zjson, zjson11, dynamodb and sqs: every case serialized, and every
    // 2xx parsed but `Ping`'s and `Measure`'s, which have no output.
    assert_eq!(counts, [38, 15]);
    Ok(())
}
