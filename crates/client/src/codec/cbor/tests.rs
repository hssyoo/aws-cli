use super::*;
use crate::codec::value::write_value;
use crate::error::ParseError;
use model::shapes::Model;
use model::value::Map;
use serde_json::json;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// One structure `S` with a member of every kind the codec writes its own
/// way, and shape types botocore's CBOR serializer has no method for.
fn model() -> Result<Model> {
    let model = json!({
        "metadata": {"apiVersion": "2020-01-01", "protocol": "smithy-rpc-v2-cbor"},
        "operations": {},
        "shapes": {
            "S": {"type": "structure", "members": {
                "Str": {"shape": "Str", "locationName": "str"},
                "Int": {"shape": "Int"}, "Bool": {"shape": "Bool"}, "Dbl": {"shape": "Dbl"},
                "Flt": {"shape": "Flt"}, "Blob": {"shape": "Blob"}, "When": {"shape": "When"},
                "L": {"shape": "L"}, "M": {"shape": "M"}, "Doc": {"shape": "Doc"},
                "Byte": {"shape": "Byte"}, "N": {"shape": "S"}}},
            "Str": {"type": "string"}, "Int": {"type": "integer"},
            "Bool": {"type": "boolean"}, "Dbl": {"type": "double"}, "Flt": {"type": "float"},
            "Blob": {"type": "blob"}, "Byte": {"type": "byte"},
            "When": {"type": "timestamp", "timestampFormat": "iso8601"},
            "L": {"type": "list", "member": {"shape": "Int"}},
            "M": {"type": "map", "key": {"shape": "Str"}, "value": {"shape": "Str"}},
            "Doc": {"type": "structure", "members": {}, "document": true}
        }
    });
    Ok(Model::from_value(&model)?)
}

fn write(model: &Model, input: &serde_json::Value) -> std::result::Result<Vec<u8>, String> {
    let root = model.schema().shape_named("S").ok_or("S")?;
    let mut serializer = CBOR.serializer();
    write_value(root, &Value::from_serde(input), serializer.as_mut())
        .map_err(|e| e.into_message())?;
    serializer.finish().map_err(|e| e.into_message())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn keep(value: &Value) -> std::result::Result<Value, ParseError> {
    Ok(value.clone())
}

fn read(text: &str) -> std::result::Result<Item, String> {
    let bytes: Vec<u8> = (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap_or(0))
        .collect();
    decode(&bytes, &keep).map_err(|error| error.message)
}

/// `CBORSerializer`'s bytes: each head as short as it fits, `None`
/// members dropped before the count, a member under its `locationName`, a
/// timestamp as tag 1 over epoch seconds whatever its format, the special
/// floats in two bytes.
#[test]
fn the_serializer_writes_botocore_s_bytes() -> Result<()> {
    let model = model()?;
    let cases = [
        (json!({}), "a0".to_owned()),
        (json!({"Str": "a", "Int": null}), "a1637374726161".into()),
        (json!({"Int": 23}), "a163496e7417".into()),
        (json!({"Int": 24}), "a163496e741818".into()),
        (json!({"Int": 65536}), "a163496e741a00010000".into()),
        (json!({"Int": -1}), "a163496e7420".into()),
        (
            json!({"Int": -4294967297_i64}),
            "a163496e743b0000000100000000".into(),
        ),
        (json!({"Bool": false}), "a164426f6f6cf4".into()),
        (json!({"Flt": 1.5}), "a163466c74fa3fc00000".into()),
        (json!({"Dbl": 1.5}), "a16344626cfb3ff8000000000000".into()),
        (json!({"Blob": "ab"}), "a164426c6f62426162".into()),
        (json!({"When": 1}), "a1645768656ec101".into()),
        (
            json!({"When": 1.5}),
            "a1645768656ec1fb3ff8000000000000".into(),
        ),
        (json!({"L": [1, null]}), "a1614c8201f6".into()),
        (json!({"M": {"k": null}}), "a1614da1616bf6".into()),
        (json!({"Doc": {"k": null}}), "a163446f63a0".into()),
        (json!({"N": {"N": {}}}), "a1614ea1614ea0".into()),
    ];
    for (input, expected) in cases {
        assert_eq!(
            write(&model, &input).map(|b| hex(&b)),
            Ok(expected),
            "{input}"
        );
    }
    let root = model.schema().shape_named("S").ok_or("S")?;
    let mut special = Map::new();
    for (key, number) in [("Dbl", f64::NAN), ("Flt", f64::NEG_INFINITY)] {
        special.insert(key.into(), Value::Float(number));
    }
    let mut serializer = CBOR.serializer();
    write_value(root, &Value::Map(special), serializer.as_mut())?;
    assert_eq!(hex(&serializer.finish()?), "a26344626cf97e0063466c74f9fc00");
    Ok(())
}

/// Python's errors for what the serializer cannot write.
#[test]
fn the_serializer_refuses_as_python_does() -> Result<()> {
    let model = model()?;
    for (input, error) in [
        (
            json!({"Byte": 1}),
            "'RpcV2CBORSerializer' object has no attribute '_serialize_type_byte'",
        ),
        (json!({"Doc": {"k": 1}}), "'k'"),
        (json!({"Doc": "x"}), "'str' object has no attribute 'items'"),
        (
            json!({"Flt": 1e39}),
            "float too large to pack with f format",
        ),
        (
            json!({"Int": 18446744073709551616_u128}),
            "int too big to convert",
        ),
        (json!({"Blob": 3}), "'int' object has no attribute 'read'"),
    ] {
        assert_eq!(write(&model, &input), Err(error.to_owned()), "{input}");
    }
    Ok(())
}

/// `parse_data_item`: integers to 64 bits either way, chunked strings, a
/// `dict`'s keys as Python compares them, tag 1 through the parser.
#[test]
fn the_decoder_reads_as_botocore_does() {
    let text = |text: &str| Item::Scalar(Value::String(text.into()));
    let big = |digits: &str| Item::Scalar(Value::BigInteger(digits.into()));
    let one = Item::Scalar(Value::Integer(1));
    for (input, expected) in [
        ("1bffffffffffffffff", big("18446744073709551615")),
        ("3bffffffffffffffff", big("-18446744073709551616")),
        ("7f61616162ff", text("ab")),
        ("5f41614162ff", Item::Scalar(Value::Blob(b"ab".to_vec()))),
        ("f93e00", Item::Scalar(Value::Float(1.5))),
        ("f90001", Item::Scalar(Value::Float(5.960464477539063e-08))),
        ("c1fb3ff8000000000000", Item::Scalar(Value::Float(1.5))),
        ("c1f5", Item::Scalar(Value::Bool(true))),
        // What follows the item is never read.
        ("0001", Item::Scalar(Value::Integer(0))),
        // `1`, `True` and `1.0` are one key, which keeps its first place.
        (
            "a3016161f56162fb3ff00000000000006163",
            Item::Map(vec![(one, text("c"))]),
        ),
        (
            "bf616101616102ff",
            Item::Map(vec![(text("a"), Item::Scalar(Value::Integer(2)))]),
        ),
    ] {
        assert_eq!(read(input), Ok(expected), "{input}");
    }
}

#[test]
fn the_decoder_fails_with_botocore_s_texts() {
    let end = "End of stream reached; this indicates a malformed CBOR response from the server \
               or an issue in botocore";
    for (input, error) in [
        ("", end),
        ("9f01", end),
        (
            "1c",
            "Invalid CBOR integer returned from the service; unparsable additional info found \
                for major type 0 or 1: 28",
        ),
        (
            "5f7f6161ffff",
            "Invalid CBOR integer returned from the service; unparsable additional \
                          info found for major type 0 or 1: 31",
        ),
        (
            "f8",
            "Invalid additional info found for major type 7: 24.  This indicates an \
                unsupported simple type or an indefinite float value",
        ),
        ("c2a0", "Found CBOR tag not supported by botocore: 2"),
        ("c1a0", "Unable to parse datetime value: {}"),
        ("c160", "Unable to parse datetime value: "),
        (
            "61ff",
            "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte",
        ),
        ("a2a00101", "unhashable type: 'dict'"),
        ("a281010101", "unhashable type: 'list'"),
        (
            "5bffffffffffffffff",
            "cannot fit 'int' into an index-sized integer",
        ),
        ("7b7fffffffffffffff", "byte string is too large"),
    ] {
        assert_eq!(read(input), Err(error.to_owned()), "{input}");
    }
    let deep = "81".repeat(MAX_DEPTH) + "00";
    assert!(read(&deep).is_ok());
    let deeper = "81".repeat(MAX_DEPTH + 1) + "00";
    assert!(read(&deeper).is_err_and(|error| error.starts_with("native client limit")));
}

/// A map key that is not text reaches no output: the call fails, naming
/// the limit; a structure looks its members up past it.
#[test]
fn a_key_that_is_not_text_is_the_native_limit() -> Result<()> {
    let item = read("a201016161f5")?;
    let error = item.to_value().map_err(|error| error.message);
    assert!(error.is_err_and(|text| text.contains("CBOR map key that is not text (1)")));
    assert_eq!(item.get("a"), Some(&Item::Scalar(Value::Bool(true))));
    Ok(())
}
