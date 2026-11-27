use super::*;
use crate::codec::value::{build_value, write_value};
use crate::protocol::ParseOptions;
use model::shapes::Model;
use serde_json::{Value as Json, json};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// One structure `S` with a member of every kind the codec writes or reads
/// its own way, and the header members `RestJSONParser` reads.
fn model() -> Result<Model> {
    let model = json!({
        "metadata": {"apiVersion": "2020-01-01", "protocol": "rest-json"},
        "operations": {},
        "shapes": {
            "S": {"type": "structure", "members": {
                "Str": {"shape": "Str", "locationName": "str"},
                "Int": {"shape": "Int"}, "Long": {"shape": "Long"},
                "Bool": {"shape": "Bool"}, "Dbl": {"shape": "Dbl"}, "Flt": {"shape": "Flt"},
                "Blob": {"shape": "Blob"}, "When": {"shape": "When"}, "Iso": {"shape": "Iso"},
                "L": {"shape": "L"}, "M": {"shape": "M"}, "Doc": {"shape": "Doc"},
                "U": {"shape": "U"}, "N": {"shape": "S"},
                "HList": {"shape": "L", "location": "header", "locationName": "x-list"},
                "HJson": {"shape": "JStr", "location": "header", "locationName": "x-json"},
                "BJson": {"shape": "JStr"}}},
            "Str": {"type": "string"}, "Int": {"type": "integer"}, "Long": {"type": "long"},
            "Bool": {"type": "boolean"}, "Dbl": {"type": "double"}, "Flt": {"type": "float"},
            "Blob": {"type": "blob"}, "When": {"type": "timestamp"},
            "Iso": {"type": "timestamp", "timestampFormat": "iso8601"},
            "JStr": {"type": "string", "jsonvalue": true},
            "L": {"type": "list", "member": {"shape": "Int"}},
            "M": {"type": "map", "key": {"shape": "Str"}, "value": {"shape": "Str"}},
            "Doc": {"type": "structure", "members": {}, "document": true},
            "U": {"type": "structure", "union": true, "members": {"A": {"shape": "Str"}}}
        }
    });
    Ok(Model::from_value(&model)?)
}

fn root(model: &Model) -> Result<Schema<'_>> {
    Ok(model.schema().shape_named("S").ok_or("S")?)
}

fn member<'m>(model: &'m Model, name: &str) -> Result<Schema<'m>> {
    Ok(root(model)?.member(name).ok_or(name.to_owned())?)
}

fn value(json: &Json) -> Value {
    Value::from_serde(json)
}

/// The tree `JSONSerializer` builds for `input` of `S`.
fn written(model: &Model, input: &Json) -> std::result::Result<Value, SerdeError> {
    let mut serializer = JsonSerializer::default();
    write_value(
        root(model).map_err(|e| SerdeError::new(e.to_string()))?,
        &value(input),
        &mut serializer,
    )?;
    serializer.into_value()
}

const IDENTITY: ParseOptions<'static> = ParseOptions {
    timestamp: &|value| Ok(value.clone()),
    blob: &|value| Ok(value.clone()),
};

/// `schema` read from `wire` through `codec`'s cursor, with identity hooks.
fn read(codec: &JsonCodec, schema: Schema<'_>, wire: &Value) -> std::result::Result<Value, String> {
    build_value(schema, &mut codec.cursor(wire), &IDENTITY).map_err(|error| error.message)
}

#[test]
fn members_go_in_the_input_order_under_their_wire_names() -> Result<()> {
    let model = model()?;
    let tree = written(
        &model,
        &json!({"M": {"z": "1", "a": null}, "Str": "s", "L": [1, null]}),
    )?;
    assert_eq!(
        tree,
        value(&json!({"M": {"z": "1", "a": null}, "str": "s", "L": [1, null]}))
    );
    let keys: Vec<String> = match &tree {
        Value::Map(map) => map.iter().map(|(key, _)| key.text().into_owned()).collect(),
        _ => Vec::new(),
    };
    assert_eq!(keys, ["M", "str", "L"]);
    Ok(())
}

#[test]
fn scalars_take_json_serializer_s_forms() -> Result<()> {
    let model = model()?;
    let input = json!({"Blob": "hi", "When": "2020-01-02T03:04:05Z", "Iso": 1.5,
        "Int": "7", "Bool": "x", "Doc": [1, {"b": null}], "N": {"When": 1.25}});
    let mut input = value(&input);
    if let Value::Map(map) = &mut input {
        map.insert("Dbl".into(), Value::Float(f64::NAN));
        map.insert("Flt".into(), Value::Float(f64::NEG_INFINITY));
        map.insert("Long".into(), Value::Float(f64::INFINITY));
    }
    let mut serializer = JsonSerializer::default();
    write_value(root(&model)?, &input, &mut serializer)?;
    let expected = json!({"Blob": "aGk=", "When": 1_577_934_245,
        "Iso": "1970-01-01T00:00:01.500000Z", "Int": "7", "Bool": "x", "Doc": [1, {"b": null}], "N": {"When": 1.25},
        "Dbl": "NaN", "Flt": "-Infinity"});
    let mut expected = value(&expected);
    // A long is no float: botocore writes it as it is, which `dumps` spells.
    if let Value::Map(map) = &mut expected {
        map.insert("Long".into(), Value::Float(f64::INFINITY));
    }
    assert_eq!(
        format!("{:?}", serializer.into_value()?),
        format!("{expected:?}")
    );
    Ok(())
}

#[test]
fn the_finished_bytes_are_json_dumps_of_the_tree() -> Result<()> {
    let model = model()?;
    let input = value(&json!({"Str": "é\"", "L": [1, 2], "M": {}}));
    let mut serializer = JSON.serializer();
    write_value(root(&model)?, &input, serializer.as_mut())?;
    let bytes = serializer.finish()?;
    assert_eq!(
        String::from_utf8(bytes)?,
        r#"{"str": "\u00e9\"", "L": [1, 2], "M": {}}"#
    );
    assert_eq!(dumps(&Value::Map(Map::new()))?, b"{}");
    assert_eq!(JSON.media_type(), "application/json");
    Ok(())
}

#[test]
fn values_a_json_body_cannot_hold_fail_with_botocore_s_text() -> Result<()> {
    let model = model()?;
    let failed = |input: Json| written(&model, &input).err().map(|e| e.into_message());
    assert_eq!(
        failed(json!({"Blob": 3})).as_deref(),
        Some("a bytes-like object is required, not 'int'")
    );
    assert_eq!(
        failed(json!({"When": "soon"})).as_deref(),
        Some("Invalid timestamp \"soon\"")
    );
    assert_eq!(failed(json!({"Nope": 1})).as_deref(), Some("'Nope'"));
    let empty = JsonSerializer::default().into_value().err();
    assert_eq!(
        empty.map(SerdeError::into_message).as_deref(),
        Some("no JSON value written")
    );
    Ok(())
}

#[test]
fn json_parser_reads_values_as_the_wire_holds_them() -> Result<()> {
    let model = model()?;
    let wire = value(
        &json!({"Int": "7", "Bool": "true", "Dbl": 1, "str": "s", "Str": "no",
        "L": [1, null], "M": {"b": null, "a": "x"}, "Doc": {"z": [null]}, "N": null,
        "When": "t", "Blob": "aGk="}),
    );
    let parsed = read(&JSON, root(&model)?, &wire)?;
    let expected = json!({"Str": "s", "Int": "7", "Bool": "true", "Dbl": 1, "Blob": "aGk=",
        "When": "t", "L": [1, null], "M": {"b": null, "a": "x"}, "Doc": {"z": [null]}});
    assert_eq!(parsed, value(&expected));
    Ok(())
}

#[test]
fn json_parser_mismatches_fail_with_python_s_text() -> Result<()> {
    let model = model()?;
    let cases = [
        ("N", json!(1), "'int' object has no attribute 'get'"),
        ("L", json!("1,2"), "'str' object is not iterable"),
        ("M", json!([1]), "'list' object has no attribute 'items'"),
        (
            "Blob",
            json!(5),
            "argument should be a bytes-like object or ASCII string, not 'int'",
        ),
    ];
    for (name, wire, text) in cases {
        let got = read(&JSON, member(&model, name)?, &value(&wire)).err();
        assert_eq!(got.as_deref(), Some(text), "{name}");
    }
    assert_eq!(read(&JSON, root(&model)?, &Value::Null)?, Value::Null);
    Ok(())
}

#[test]
fn a_union_s_tags_are_its_set_wire_keys() -> Result<()> {
    let model = model()?;
    let wire = value(&json!({"A": null, "B": 1, "__type": "x"}));
    assert_eq!(
        JSON.cursor(&wire).union_tags(),
        Some(vec![Key::from("B"), Key::from("__type")])
    );
    assert_eq!(JSON.cursor(&value(&json!([]))).union_tags(), None);
    let unknown = read(&JSON, member(&model, "U")?, &wire)?;
    assert_eq!(
        unknown,
        value(&json!({"SDK_UNKNOWN_MEMBER": {"name": "B"}}))
    );
    Ok(())
}

#[test]
fn rest_json_parser_converts_scalars_as_python_does() -> Result<()> {
    let model = model()?;
    let wire = value(
        &json!({"Int": "7", "Long": 2.9, "Bool": "TRUE", "Dbl": "1e3",
        "Flt": 2, "L": ["3", true], "BJson": "eyJhIjogMX0="}),
    );
    let parsed = read(&REST_JSON, root(&model)?, &wire)?;
    let expected = json!({"Int": 7, "Long": 2, "Bool": true, "Dbl": 1000.0, "Flt": 2.0,
        "L": [3, 1], "BJson": "eyJhIjogMX0="});
    assert_eq!(format!("{parsed:?}"), format!("{:?}", value(&expected)));
    let failed = |name: &str, wire: Json| -> Result<String> {
        Ok(read(&REST_JSON, member(&model, name)?, &value(&wire))
            .err()
            .ok_or("parsed")?)
    };
    assert_eq!(
        failed("Int", json!("1.5"))?,
        "invalid literal for int() with base 10: '1.5'"
    );
    assert_eq!(
        failed("Dbl", json!("x"))?,
        "could not convert string to float: 'x'"
    );
    assert_eq!(
        failed("Int", json!({}))?,
        "int() argument must be a string, a bytes-like object or a real number, not 'dict'"
    );
    assert_eq!(
        failed("Flt", json!([]))?,
        "float() argument must be a string or a real number, not 'list'"
    );
    Ok(())
}

#[test]
fn rest_json_parser_splits_header_lists_and_reads_json_value_headers() -> Result<()> {
    let model = model()?;
    let header = |name, text: &str| {
        read(
            &REST_JSON,
            member(&model, name)?,
            &Value::String(text.to_owned()),
        )
        .map_err(Box::<dyn std::error::Error>::from)
    };
    assert_eq!(header("HList", " 1, 2 ,3")?, value(&json!([1, 2, 3])));
    assert_eq!(
        header("HJson", "eyJhIjogWzFdfQ==")?,
        value(&json!({"a": [1]}))
    );
    assert_eq!(
        header("HJson", "é").err().map(|e| e.to_string()).as_deref(),
        Some("string argument should contain only ASCII characters")
    );
    let status = read(&REST_JSON, member(&model, "HList")?, &Value::Integer(5)).err();
    assert_eq!(
        status.as_deref(),
        Some("'int' object has no attribute 'split'")
    );
    // `JSONParser` has neither handler: the text stays as it is.
    let plain = read(
        &JSON,
        member(&model, "HJson")?,
        &Value::String("eyJ9".into()),
    )?;
    assert_eq!(plain, Value::String("eyJ9".into()));
    let unsplit = read(
        &JSON,
        member(&model, "HList")?,
        &Value::String("1,2".into()),
    )
    .err();
    assert_eq!(unsplit.as_deref(), Some("'str' object is not iterable"));
    Ok(())
}

#[test]
fn a_body_loads_as_parse_body_as_json_reads_it() -> Result<()> {
    assert_eq!(load(b"")?, Value::Map(Map::new()));
    assert_eq!(load(b"null")?, Value::Null);
    assert_eq!(load(b"{bad")?, value(&json!({"message": "{bad"})));
    let big = load(b"[12345678901234567890123, 1.0]")?;
    assert_eq!(
        big,
        Value::List(vec![
            Value::BigInteger("12345678901234567890123".into()),
            Value::Float(1.0)
        ])
    );
    let depth = model::pyjson::MAX_DEPTH + 1;
    let deep = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    let failed = load(deep.as_bytes()).err().map(|error| error.message);
    assert!(
        failed
            .as_deref()
            .is_some_and(|text| text.starts_with("native client limit: ")),
        "{failed:?}"
    );
    let undecodable = load(b"\"\xff\"").err().map(|error| error.message);
    assert_eq!(
        undecodable.as_deref(),
        Some("'utf-8' codec can't decode byte 0xff in position 1: invalid start byte")
    );
    Ok(())
}

/// The codec's own deserializer over bytes reads as a cursor over
/// [`load`]'s tree does, with each codec's handlers.
#[test]
fn the_codec_s_deserializer_reads_its_loaded_body() -> Result<()> {
    let model = model()?;
    let body = br#"{"Int": "7", "N": {"Bool": "true"}, "U": {"A": "a"}, "L": ["1"]}"#;
    for codec in [&JSON, &REST_JSON] {
        let mut deserializer = codec.deserializer(body)?;
        assert!(!deserializer.is_null());
        assert!(deserializer.union_tags().is_some());
        let parsed = build_value(root(&model)?, deserializer.as_mut(), &IDENTITY)?;
        assert_eq!(
            Ok(parsed),
            read(codec, root(&model)?, &load(body)?),
            "{codec:?}"
        );
    }
    let mut empty = REST_JSON.deserializer(b"")?;
    assert_eq!(
        build_value(root(&model)?, empty.as_mut(), &IDENTITY)?,
        Value::Map(Map::new())
    );
    let bad = JSON
        .deserializer(b"\xff")
        .err()
        .map(SerdeError::into_message);
    assert!(bad.is_some_and(|text| text.starts_with("'utf-8' codec")));
    Ok(())
}
