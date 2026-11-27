use super::*;
use crate::error::ParseError;
use model::shapes::Model;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const MODEL: &str = r#"{
  "metadata": {"apiVersion": "2020-01-01", "protocol": "query"},
  "operations": {},
  "shapes": {
    "Out": {"type": "structure", "members": {
      "Name": {"shape": "S", "locationName": "name"},
      "Count": {"shape": "I"}, "On": {"shape": "B"}, "Ratio": {"shape": "F"},
      "At": {"shape": "T"}, "Data": {"shape": "Blob"},
      "Items": {"shape": "List"}, "Flat": {"shape": "FlatList"},
      "Tags": {"shape": "Map"}, "FlatTags": {"shape": "FlatMap"},
      "Header": {"shape": "S", "location": "header", "locationName": "x-h"},
      "Kind": {"shape": "S", "xmlAttribute": true, "locationName": "xsi:type"},
      "Choice": {"shape": "U"}, "Doc": {"shape": "D"}
    }},
    "S": {"type": "string"},
    "I": {"type": "integer"},
    "B": {"type": "boolean"},
    "F": {"type": "double"},
    "T": {"type": "timestamp"},
    "Blob": {"type": "blob"},
    "List": {"type": "list", "member": {"shape": "S", "locationName": "item"}},
    "FlatList": {"type": "list", "member": {"shape": "S", "locationName": "Entry"},
      "flattened": true},
    "Map": {"type": "map", "key": {"shape": "S"}, "value": {"shape": "I"}},
    "FlatMap": {"type": "map", "key": {"shape": "S", "locationName": "K"},
      "value": {"shape": "S", "locationName": "V"}, "flattened": true},
    "U": {"type": "structure", "union": true, "members": {"A": {"shape": "S"}}},
    "D": {"type": "structure", "members": {}, "document": true},
    "Fault": {"type": "structure", "members": {"Detail": {"shape": "S"}},
      "error": {"code": "Fault"}, "exception": true}
  }
}"#;

fn model() -> Result<Model> {
    Ok(Model::from_json(MODEL.as_bytes())?)
}

fn shape<'m>(model: &'m Model, name: &str) -> Result<Schema<'m>> {
    Ok(model.schema().shape_named(name).ok_or(name.to_owned())?)
}

/// The value `body`'s root holds as `name`, under botocore's hooks.
fn read(model: &Model, name: &str, body: &str) -> std::result::Result<Value, ParseError> {
    let schema = model
        .schema()
        .shape_named(name)
        .ok_or_else(|| ParseError::new(name))?;
    let mut deserializer = XML.deserializer(body.as_bytes())?;
    build_value(schema, &mut *deserializer, &ParseOptions::BOTOCORE)
}

fn map(json: &str) -> Value {
    Value::from_json(json.as_bytes()).unwrap_or(Value::Null)
}

/// Members in the model's order by their XML names, scalars converted as
/// the query parser converts them, header members and unknown elements
/// left out, an attribute's text as it is.
#[test]
fn a_structure_reads_members_in_model_order() -> Result<()> {
    let model = model()?;
    let body = concat!(
        "<R xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:type=\"k\">",
        "<Ratio>1.5</Ratio><x-h>no</x-h><Header>no</Header><On>yes</On><Count> 7 </Count>",
        "<name>n</name><Other>o</Other><Doc><Any>a</Any></Doc></R>"
    );
    let parsed = read(&model, "Out", body)?;
    let expected = map(
        r#"{"Name": "n", "Count": 7, "On": false, "Ratio": 1.5, "Kind": "k",
        "Doc": {}}"#,
    );
    assert_eq!(parsed, expected);
    let Value::Map(parsed) = parsed else {
        return Err("not a map".into());
    };
    let keys: Vec<String> = parsed.keys().map(|key| key.text().into_owned()).collect();
    assert_eq!(keys, ["Name", "Count", "On", "Ratio", "Kind", "Doc"]);
    Ok(())
}

/// A timestamp's and a blob's text go to the hooks; an empty element is
/// `''`.
#[test]
fn timestamps_and_blobs_reach_the_hooks_raw() -> Result<()> {
    let model = model()?;
    let schema = shape(&model, "Out")?;
    let body = "<R><At>2020-01-02T03:04:05Z</At><Data>aGk=</Data><name/></R>";
    let mut deserializer = XML.deserializer(body.as_bytes())?;
    let raw = ParseOptions {
        timestamp: &|value| Ok(Value::List(vec![value.clone()])),
        blob: &|value| Ok(Value::List(vec![value.clone()])),
    };
    let parsed = build_value(schema, &mut *deserializer, &raw)?;
    let expected = map(r#"{"Name": "", "At": ["2020-01-02T03:04:05Z"], "Data": ["aGk="]}"#);
    assert_eq!(parsed, expected);
    Ok(())
}

/// The conversion failures' texts: Python's `float()` and `int()`, and a
/// repeated element where one is read.
#[test]
fn a_scalar_fails_with_python_s_text() -> Result<()> {
    let model = model()?;
    let failure = |body: &str| read(&model, "Out", body).err().map(|error| error.message);
    assert_eq!(
        failure("<R><Ratio>x</Ratio></R>").as_deref(),
        Some("could not convert string to float: 'x'")
    );
    assert_eq!(
        failure("<R><Count>1.5</Count></R>").as_deref(),
        Some("invalid literal for int() with base 10: '1.5'")
    );
    assert_eq!(
        failure("<R><On>true</On><On>true</On></R>").as_deref(),
        Some("expected one element, found a repeated one")
    );
    Ok(())
}

/// A list's children whatever their names; a flattened list's repeated
/// elements, named by its member.
#[test]
fn lists_read_children_or_repeated_elements() -> Result<()> {
    let model = model()?;
    let body = "<R><Entry>a</Entry><Items><item>1</item><other>2</other></Items>\
        <Entry>b</Entry></R>";
    let parsed = read(&model, "Out", body)?;
    assert_eq!(parsed, map(r#"{"Items": ["1", "2"], "Flat": ["a", "b"]}"#));
    Ok(())
}

/// Map entries by the key's and value's names, flattened without `entry`;
/// a repeated key keeps its first place.
#[test]
fn maps_read_entries() -> Result<()> {
    let model = model()?;
    let body = "<R><Tags><entry><key>b</key><value>1</value></entry>\
        <entry><key>a</key><value>2</value></entry><entry><key>b</key><value>3</value></entry>\
        </Tags><FlatTags><K>k</K><V>v</V></FlatTags></R>";
    let parsed = read(&model, "Out", body)?;
    assert_eq!(
        parsed,
        map(r#"{"Tags": {"b": 3, "a": 2}, "FlatTags": {"k": "v"}}"#)
    );
    Ok(())
}

/// `_handle_map`'s loop variables outlive an entry: an entry missing its
/// key or value takes the previous entry's, a value before its key is
/// that key's, and the last of two values wins.
#[test]
fn a_map_entry_missing_a_part_keeps_the_previous_one() -> Result<()> {
    let model = model()?;
    let body = "<R><Tags><entry><key>a</key><value>1</value></entry>\
        <entry><value>2</value></entry><entry><key>b</key></entry>\
        <entry><value>3</value><key>c</key></entry><entry><key>d</key><value>4</value>\
        <value>5</value></entry></Tags></R>";
    let parsed = read(&model, "Out", body)?;
    assert_eq!(parsed, map(r#"{"Tags": {"a": 2, "b": 2, "c": 3, "d": 5}}"#));
    Ok(())
}

/// The first failure in document order: a value that fails before an
/// unknown tag, the unknown tag before a later bad value, and a missing key
/// after the values parse.
#[test]
fn a_map_fails_where_the_loop_does() -> Result<()> {
    let model = model()?;
    let failure = |entries: &str| {
        let body = format!("<R><Tags>{entries}</Tags></R>");
        read(&model, "Out", &body).err().map(|error| error.message)
    };
    let bad_int = "invalid literal for int() with base 10: 'x'";
    assert_eq!(
        failure("<entry><value>x</value><junk/></entry>").as_deref(),
        Some(bad_int)
    );
    assert_eq!(
        failure("<entry><key>a</key><junk/><value>x</value></entry>").as_deref(),
        Some("Unknown tag: junk")
    );
    assert_eq!(
        failure("<entry><value>x</value></entry>").as_deref(),
        Some(bad_int)
    );
    assert_eq!(
        failure("<entry><value>1</value></entry>").as_deref(),
        Some("cannot access local variable 'key_name'")
    );
    assert_eq!(
        failure("<entry><key>a</key></entry>").as_deref(),
        Some("cannot access local variable 'key_name'")
    );
    Ok(())
}

/// A union's one set member; one the model does not know is
/// `SDK_UNKNOWN_MEMBER`, and two set is the union error.
#[test]
fn a_union_reads_its_tags() -> Result<()> {
    let model = model()?;
    let union = |inner: &str| read(&model, "Out", &format!("<R><Choice>{inner}</Choice></R>"));
    assert_eq!(union("<A>a</A>")?, map(r#"{"Choice": {"A": "a"}}"#));
    assert_eq!(
        union("<New>n</New>")?,
        map(r#"{"Choice": {"SDK_UNKNOWN_MEMBER": {"name": "New"}}}"#)
    );
    assert_eq!(
        union("<A>a</A><New>n</New>")
            .err()
            .map(|error| error.message),
        Some("Invalid service response: U must have one and only one member set.".to_owned())
    );
    Ok(())
}

/// An error shape reads from an `ErrorResponse`'s `Error`.
#[test]
fn an_error_shape_reads_from_the_error_element() -> Result<()> {
    let model = model()?;
    let body = "<ErrorResponse><Error><Detail>d</Detail></Error><Detail>no</Detail>\
        </ErrorResponse>";
    assert_eq!(read(&model, "Fault", body)?, map(r#"{"Detail": "d"}"#));
    Ok(())
}

/// A body that is not XML is `ResponseParserError`'s text, with expat's
/// message and the body as bytes.
#[test]
fn malformed_xml_is_botocore_s_parser_error() {
    let failed = XML.deserializer(b"<a></b>").err();
    let expected = "Unable to parse response (mismatched tag: line 1, column 5), invalid XML \
        received. Further retries may succeed:\nb'<a></b>'";
    assert_eq!(failed.as_ref().map(SerdeError::message), Some(expected));
}

/// `_build_name_to_xml_node` groups a repeated name in first place, and
/// `_replace_nodes` gives text, `None`, or a dict.
#[test]
fn the_name_mapping_and_its_replacement() -> Result<()> {
    let root = parse(b"<r><a>1</a><b><c>2</c></b><a>3</a><d/></r>")?;
    let mapping = by_name(&root);
    let names: Vec<&str> = mapping.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["a", "b", "d"]);
    assert!(matches!(lookup(&mapping, "a"), Some(Node::Many(items)) if items.len() == 2));
    let replaced = replaced(mapping);
    assert_eq!(
        Value::Map(replaced),
        map(r#"{"a": {}, "b": {"c": "2"}, "d": null}"#)
    );
    Ok(())
}

/// The codec writes too: the root element is named by the `locationName`
/// of the shape first written, whose absence is `KeyError: 'name'`.
#[test]
fn the_xml_codec_serializes() -> Result<()> {
    let model = model()?;
    assert_eq!(XML.media_type(), "text/xml");
    let name = shape(&model, "Out")?.member("Name").ok_or("Name")?;
    let mut serializer = XML.serializer();
    serializer.write_string(name, &Value::String("a&b".into()))?;
    assert_eq!(serializer.finish()?, b"<name>a&amp;b</name>");
    let mut serializer = XML.serializer();
    let refused = serializer.write_string(shape(&model, "S")?, &Value::String("s".into()));
    assert_eq!(refused.as_ref().map_err(SerdeError::message), Err("'name'"));
    assert!(serializer.finish().is_err());
    Ok(())
}
