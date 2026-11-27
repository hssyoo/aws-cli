use super::*;
use model::shapes::Model;
use model::value::Map;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const MODEL: &str = r#"{
  "metadata": {"apiVersion": "2020-01-01", "protocol": "query"},
  "operations": {},
  "shapes": {
    "In": {"type": "structure", "members": {
      "Name": {"shape": "S", "locationName": "name"},
      "Inner": {"shape": "In"},
      "Items": {"shape": "List"}, "Named": {"shape": "NamedList"},
      "Flat": {"shape": "FlatList"}, "FlatBare": {"shape": "FlatBareList"},
      "Tags": {"shape": "Map"}, "FlatTags": {"shape": "FlatMap"},
      "Numbers": {"shape": "IntMap"}, "Nested": {"shape": "Nested"},
      "On": {"shape": "B"}, "Ratio": {"shape": "F"}, "Count": {"shape": "I"},
      "At": {"shape": "T"}, "Epoch": {"shape": "Epoch"}, "Data": {"shape": "Blob"},
      "Doc": {"shape": "D"}
    }},
    "S": {"type": "string"},
    "I": {"type": "integer"},
    "B": {"type": "boolean"},
    "F": {"type": "double"},
    "T": {"type": "timestamp"},
    "Epoch": {"type": "timestamp", "timestampFormat": "unixTimestamp"},
    "Blob": {"type": "blob"},
    "List": {"type": "list", "member": {"shape": "S"}},
    "NamedList": {"type": "list", "member": {"shape": "S", "locationName": "item"}},
    "FlatList": {"type": "list", "member": {"shape": "S", "locationName": "Entry"},
      "flattened": true},
    "FlatBareList": {"type": "list", "member": {"shape": "S"}, "flattened": true},
    "Nested": {"type": "list", "member": {"shape": "In"}},
    "Map": {"type": "map", "key": {"shape": "S"}, "value": {"shape": "S"}},
    "IntMap": {"type": "map", "key": {"shape": "S", "locationName": "K"},
      "value": {"shape": "I", "locationName": "V"}},
    "FlatMap": {"type": "map", "key": {"shape": "S"}, "value": {"shape": "S"},
      "flattened": true},
    "D": {"type": "structure", "members": {}, "document": true}
  }
}"#;

fn model() -> Result<Model> {
    Ok(Model::from_json(MODEL.as_bytes())?)
}

fn input(json: &str) -> Value {
    Value::from_json(json.as_bytes()).unwrap_or(Value::Null)
}

/// The body `value` serializes to as an `In`, through the codec's own
/// serializer.
fn body(model: &Model, value: &Value) -> std::result::Result<String, SerdeError> {
    let schema = model
        .schema()
        .shape_named("In")
        .ok_or_else(|| SerdeError::new("In"))?;
    let mut serializer = FORM.serializer();
    write_value(schema, value, &mut *serializer)?;
    let bytes = serializer.finish()?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// A structure's members after its prefix, by `locationName` else name,
/// in the input's order.
#[test]
fn structures_prefix_their_members() -> Result<()> {
    let model = model()?;
    let written = body(
        &model,
        &input(r#"{"Inner": {"Inner": {"Name": "deep"}, "Name": "in"}, "Name": "top"}"#),
    )?;
    assert_eq!(written, "Inner.Inner.name=deep&Inner.name=in&name=top");
    Ok(())
}

/// `member.N`, or the member's `locationName`; a flattened list's member
/// name replaces the last part of its prefix, and an empty list is `Name=`.
#[test]
fn lists_number_their_items() -> Result<()> {
    let model = model()?;
    let written = body(
        &model,
        &input(
            r#"{"Items": ["a", "b"], "Named": ["c"], "Flat": ["d"], "FlatBare": ["e"],
               "Inner": {"Flat": ["f"]}, "Nested": [{"Name": "g"}, {"Items": []}]}"#,
        ),
    )?;
    assert_eq!(
        written,
        "Items.member.1=a&Items.member.2=b&Named.item.1=c&Entry.1=d&FlatBare.1=e\
         &Inner.Entry.1=f&Nested.member.1.name=g&Nested.member.2.Items="
    );
    Ok(())
}

/// `entry.N.key` and `entry.N.value` with the key's and value's names, no
/// `entry` when flattened; the key and value are written through their
/// shapes.
#[test]
fn maps_number_their_entries() -> Result<()> {
    let model = model()?;
    let written = body(
        &model,
        &input(r#"{"Tags": {"a": "1", "b c": "2"}, "FlatTags": {"k": "v"}, "Numbers": {"n": 3}}"#),
    )?;
    assert_eq!(
        written,
        "Tags.entry.1.key=a&Tags.entry.1.value=1&Tags.entry.2.key=b+c&Tags.entry.2.value=2\
         &FlatTags.1.key=k&FlatTags.1.value=v&Numbers.entry.1.K=n&Numbers.entry.1.V=3"
    );
    Ok(())
}

/// Scalars: `str(value)` whatever the type says, a boolean's truthiness, a
/// special float by name, a timestamp in its format, a blob in base64.
#[test]
fn scalars_are_written_as_the_query_serializer_writes_them() -> Result<()> {
    let model = model()?;
    let mut fields = Map::new();
    fields.insert("On".into(), Value::String(String::new()));
    fields.insert("Ratio".into(), Value::Float(f64::NEG_INFINITY));
    fields.insert("Count".into(), Value::Float(2.5));
    fields.insert("At".into(), Value::Integer(0));
    fields.insert("Epoch".into(), Value::String("1970-01-01T00:00:01Z".into()));
    fields.insert("Data".into(), Value::Blob(b"hi".to_vec()));
    fields.insert("Name".into(), Value::Integer(5));
    let written = body(&model, &Value::Map(fields))?;
    assert_eq!(
        written,
        "On=false&Ratio=-Infinity&Count=2.5&At=1970-01-01T00%3A00%3A00Z&Epoch=1&Data=aGk%3D\
         &name=5"
    );
    Ok(())
}

/// `None` in a list goes through the item's shape like any value: a
/// string's `str(None)`, a structure's `items` failure.
#[test]
fn a_null_item_is_serialized_through_its_shape() -> Result<()> {
    let model = model()?;
    assert_eq!(
        body(&model, &input(r#"{"Items": [null], "Tags": {"k": null}}"#))?,
        "Items.member.1=None&Tags.entry.1.key=k&Tags.entry.1.value=None"
    );
    assert_eq!(
        body(&model, &input(r#"{"Nested": [null]}"#))
            .err()
            .map(SerdeError::into_message),
        Some("'NoneType' object has no attribute 'items'".to_owned())
    );
    Ok(())
}

/// A document is a structure to the query serializer: a dict's members
/// (it has none, so any key is the `KeyError`), anything else the `items`
/// failure.
#[test]
fn a_document_is_written_as_a_structure() -> Result<()> {
    let model = model()?;
    let failure = |json: &str| {
        body(&model, &input(json))
            .err()
            .map(SerdeError::into_message)
    };
    assert_eq!(body(&model, &input(r#"{"Doc": {}}"#))?, "");
    assert_eq!(failure(r#"{"Doc": {"k": 1}}"#).as_deref(), Some("'k'"));
    assert_eq!(
        failure(r#"{"Doc": [1]}"#).as_deref(),
        Some("'list' object has no attribute 'items'")
    );
    Ok(())
}

/// The failures the serializer's conversions raise, with botocore's text.
#[test]
fn conversions_fail_with_botocore_s_text() -> Result<()> {
    let model = model()?;
    let failure = |json: &str| {
        body(&model, &input(json))
            .err()
            .map(SerdeError::into_message)
    };
    assert_eq!(
        failure(r#"{"Data": 1}"#).as_deref(),
        Some("a bytes-like object is required, not 'int'")
    );
    assert_eq!(
        failure(r#"{"At": "soon"}"#).as_deref(),
        Some("Invalid timestamp \"soon\"")
    );
    assert_eq!(
        failure(r#"{"Items": "a"}"#).as_deref(),
        Some("a list is required")
    );
    Ok(())
}

/// Setting a field again replaces its value in place, as the dict does,
/// and a lone surrogate fails `urlencode` at its position.
#[test]
fn fields_are_a_dict_until_they_are_encoded() -> Result<()> {
    let model = model()?;
    let schema = model.schema().shape_named("In").ok_or("In")?;
    let mut form = FormSerializer::default();
    form.set("name", b"first".to_vec());
    form.set("Action", b"Go".to_vec());
    write_value(schema, &input(r#"{"Name": "second"}"#), &mut form)?;
    assert_eq!(Box::new(form).finish()?, b"name=second&Action=Go");
    let mut form = FormSerializer::default();
    form.set("Text", vec![0x61, 0xED, 0xB2, 0x80]);
    assert_eq!(
        Box::new(form).finish().err().map(SerdeError::into_message),
        Some(
            "'utf-8' codec can't encode character '\\udc80' in position 1: surrogates not allowed"
                .to_owned()
        )
    );
    Ok(())
}

/// The codec writes only.
#[test]
fn the_form_codec_does_not_deserialize() {
    assert_eq!(
        FORM.media_type(),
        "application/x-www-form-urlencoded; charset=utf-8"
    );
    assert!(FORM.deserializer(b"a=b").is_err());
}
