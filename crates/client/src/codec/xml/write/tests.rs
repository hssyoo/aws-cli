//! The writer's quirks neither botocore's protocol suite nor the fakes
//! reach, each expectation what botocore's
//! `RestXMLSerializer._serialize_body_params` gives for the same model and
//! input (CPython 3.12's ElementTree, whose writer 3.14's matches).
use super::*;
use crate::codec::value::write_value;
use model::shapes::Model;
use model::value::Map;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const MODEL: &str = r#"{
  "metadata": {"apiVersion": "2020-01-01", "protocol": "rest-xml", "endpointPrefix": "svc"},
  "operations": {"Put": {"name": "Put", "http": {"method": "PUT", "requestUri": "/p"},
    "input": {"shape": "In", "locationName": "In"}}},
  "shapes": {
    "In": {"type": "structure", "members": {
      "A": {"shape": "S"}, "B": {"shape": "S"}, "C": {"shape": "S"},
      "Flat": {"shape": "NsFlat"}, "Map": {"shape": "FlatMap"}, "Ns": {"shape": "NsMap"},
      "Items": {"shape": "BoolList"}, "Texts": {"shape": "SL"},
      "Attr": {"shape": "I", "xmlAttribute": true, "locationName": "n"},
      "Pre": {"shape": "S", "xmlNamespace": {"prefix": "p", "uri": "https://p/"}},
      "Empty": {"shape": "S", "xmlNamespace": {"prefix": "", "uri": "https://e/"}},
      "NoUri": {"shape": "S", "xmlNamespace": {"prefix": "x"}}}},
    "S": {"type": "string"}, "I": {"type": "integer"}, "B": {"type": "boolean"},
    "SL": {"type": "list", "member": {"shape": "S"}},
    "BoolList": {"type": "list", "member": {"shape": "B", "locationName": "b"}},
    "NsFlat": {"type": "list", "member": {"shape": "S"}, "flattened": true,
      "xmlNamespace": "https://f/"},
    "FlatMap": {"type": "map", "key": {"shape": "S", "xmlNamespace": "https://k/"},
      "value": {"shape": "S", "locationName": "v"}, "flattened": true},
    "NsMap": {"type": "map", "key": {"shape": "S"}, "value": {"shape": "S"},
      "xmlNamespace": "https://m/"}
  }
}"#;

/// The body `input` serializes to, or the serializer's error text.
fn body(input: &Value) -> Result<std::result::Result<String, String>> {
    let model = Model::from_json(MODEL.as_bytes())?;
    let service = model.schema().service();
    let shape = service
        .operation("Put")
        .and_then(|op| op.input())
        .ok_or("In")?;
    let mut serializer = XmlSerializer::default();
    let written =
        write_value(shape, input, &mut serializer).and_then(|()| Box::new(serializer).finish());
    Ok(written
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .map_err(|error| error.message().to_owned()))
}

fn input(json: &str) -> Value {
    Value::from_json(json.as_bytes()).unwrap_or(Value::Null)
}

/// `_serialize_type_structure` returns at a `None` member: the members
/// after it are not written.
#[test]
fn a_none_member_ends_the_structure() -> Result<()> {
    let written = body(&input(r#"{"A": "a", "B": null, "C": "c"}"#))?;
    assert_eq!(written.as_deref(), Ok("<In><A>a</A></In>"));
    Ok(())
}

/// A flattened list's items go in the parent, which takes the list's
/// namespace.
#[test]
fn a_flattened_list_puts_its_namespace_on_the_parent() -> Result<()> {
    let written = body(&input(r#"{"A": "a", "Flat": ["x", "y"]}"#))?;
    let expected = r#"<In xmlns="https://f/"><A>a</A><Flat>x</Flat><Flat>y</Flat></In>"#;
    assert_eq!(written.as_deref(), Ok(expected));
    Ok(())
}

/// A flattened map is one element per entry, named as the map; a key's
/// namespace is on each key; a map's own namespace on its element.
#[test]
fn maps_flattened_and_namespaced() -> Result<()> {
    let written = body(&input(r#"{"Map": {"k1": "1", "k2": "2"}, "C": "c"}"#))?;
    let expected = concat!(
        r#"<In><Map><key xmlns="https://k/">k1</key><v>1</v></Map>"#,
        r#"<Map><key xmlns="https://k/">k2</key><v>2</v></Map><C>c</C></In>"#
    );
    assert_eq!(written.as_deref(), Ok(expected));
    let written = body(&input(r#"{"Ns": {"k": "v"}}"#))?;
    let expected =
        r#"<In><Ns xmlns="https://m/"><entry><key>k</key><value>v</value></entry></Ns></In>"#;
    assert_eq!(written.as_deref(), Ok(expected));
    Ok(())
}

/// A `None` list item goes through its shape's handler: a boolean by its
/// truthiness, text as `None`; an empty text is an empty element.
#[test]
fn none_list_items_and_truthiness() -> Result<()> {
    let written = body(&input(
        r#"{"Items": [null, true, 0, "x"], "Texts": [null, ""]}"#,
    ))?;
    let expected = concat!(
        "<In><Items><b>false</b><b>true</b><b>false</b><b>true</b></Items>",
        "<Texts><member>None</member><member /></Texts></In>"
    );
    assert_eq!(written.as_deref(), Ok(expected));
    Ok(())
}

/// A lone surrogate is a character reference (`xmlcharrefreplace`); a
/// prefix names the `xmlns:` attribute, an empty one is the default.
#[test]
fn surrogates_and_prefixes() -> Result<()> {
    let points = "a"
        .chars()
        .map(u32::from)
        .chain([0xd800])
        .chain(['b'.into()]);
    let mut map = Map::new();
    map.insert(
        "A".into(),
        Value::from_text(pytext::wtf8::Wtf8Buf::from_code_points(points)),
    );
    map.insert("Pre".into(), Value::String("p".into()));
    map.insert("Empty".into(), Value::String("e".into()));
    let written = body(&Value::Map(map))?;
    let expected = concat!(
        r#"<In><A>a&#55296;b</A><Pre xmlns:p="https://p/">p</Pre>"#,
        r#"<Empty xmlns="https://e/">e</Empty></In>"#
    );
    assert_eq!(written.as_deref(), Ok(expected));
    Ok(())
}

/// An attribute holds what it was given, which the writer refuses unless
/// it is text; a namespace without a `uri` is `KeyError: 'uri'`.
#[test]
fn what_the_writer_refuses() -> Result<()> {
    let refused = body(&input(r#"{"Attr": 5}"#))?;
    assert_eq!(refused, Err("cannot serialize 5 (type int)".to_owned()));
    let refused = body(&input(r#"{"NoUri": "u"}"#))?;
    assert_eq!(refused, Err("'uri'".to_owned()));
    Ok(())
}
