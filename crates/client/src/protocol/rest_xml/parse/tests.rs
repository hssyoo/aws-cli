//! `RestXMLParser`'s quirks the protocol suite and the fakes do not pin,
//! each expectation what botocore gives for the same model and response.
use super::*;
use crate::http::Headers;
use crate::protocol::REST_XML;
use crate::protocol::compat;
use model::shapes::Model;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const MODEL: &str = r#"{
  "metadata": {"apiVersion": "2020-01-01", "protocol": "rest-xml", "endpointPrefix": "svc"},
  "operations": {"Get": {"name": "Get", "http": {"method": "GET", "requestUri": "/g"},
    "output": {"shape": "Out"}}},
  "shapes": {
    "Out": {"type": "structure", "members": {
      "Status": {"shape": "I", "location": "statusCode"},
      "Names": {"shape": "SL", "location": "header", "locationName": "x-names"},
      "Flags": {"shape": "BL", "location": "header", "locationName": "x-flags"},
      "Ratio": {"shape": "D", "location": "header", "locationName": "x-ratio"},
      "Json": {"shape": "S", "location": "header", "locationName": "x-json", "jsonvalue": true},
      "Body": {"shape": "S"}}},
    "S": {"type": "string"}, "I": {"type": "integer"}, "B": {"type": "boolean"},
    "D": {"type": "double"},
    "SL": {"type": "list", "member": {"shape": "S"}},
    "BL": {"type": "list", "member": {"shape": "B"}}
  }
}"#;

fn response(status: u16, headers: &[(&str, &str)], body: &str) -> HttpResponse {
    HttpResponse {
        status,
        headers: Headers::from_pairs(headers.iter().copied()),
        body: body.as_bytes().to_vec(),
    }
}

/// The parsed output of `response` for `Get`, `ResponseMetadata` aside.
fn parsed(response: &HttpResponse) -> Result<std::result::Result<Value, String>> {
    let model = Model::from_json(MODEL.as_bytes())?;
    let operation = model.schema().service().operation("Get").ok_or("Get")?;
    let parsed = compat::parse(
        &REST_XML,
        operation,
        operation.output(),
        response,
        &ParseOptions::BOTOCORE,
    );
    Ok(parsed
        .map(|value| match value {
            Value::Map(mut map) => {
                map.remove("ResponseMetadata");
                Value::Map(map)
            }
            other => other,
        })
        .map_err(|error| error.message))
}

fn json(text: &str) -> Value {
    Value::from_json(text.as_bytes()).unwrap_or(Value::Null)
}

/// The status code as an integer; a header list split at commas and
/// stripped, a boolean `true` only as written in lower case, a float as
/// `float()` reads it, a `jsonvalue` header decoded.
#[test]
fn the_status_and_headers_through_the_xml_handlers() -> Result<()> {
    let headers = [
        ("x-names", " a , b,c"),
        ("x-flags", "true,True"),
        ("x-ratio", "1e3"),
        ("x-json", "eyJhIjogWzFdfQ=="),
    ];
    let parsed = parsed(&response(202, &headers, "<Out><Body>t</Body></Out>"))?;
    let expected = json(
        r#"{"Status": 202, "Names": ["a", "b", "c"], "Flags": [true, false],
        "Ratio": 1000.0, "Json": {"a": [1]}, "Body": "t"}"#,
    );
    assert_eq!(parsed, Ok(expected));
    Ok(())
}

/// `merge_dicts` of an envelope whose `Message` or `Code` has children
/// fails as item assignment on the default's `''` does; an empty one is
/// `None`.
#[test]
fn an_envelope_merge_into_text_fails() -> Result<()> {
    let failure = "'str' object does not support item assignment".to_owned();
    for body in [
        "<ErrorResponse><Error><Message><X>1</X></Message></Error></ErrorResponse>",
        "<ErrorResponse><Error><Message><X/></Message><Code><Y/></Code></Error></ErrorResponse>",
    ] {
        assert_eq!(parsed(&response(400, &[], body))?, Err(failure.clone()));
    }
    let body = "<ErrorResponse><Error><Message></Message></Error></ErrorResponse>";
    let parsed = parsed(&response(400, &[], body))?;
    assert_eq!(
        parsed,
        Ok(json(r#"{"Error": {"Message": null, "Code": ""}}"#))
    );
    Ok(())
}

/// A header read as a structure or a map is not an XML node.
#[test]
fn a_header_is_no_node() -> Result<()> {
    let model = Model::from_json(MODEL.as_bytes())?;
    let shape = model.schema().shape_named("Out").ok_or("Out")?;
    let text = Value::String("ab".into());
    let failed = HeaderValue(&text).read_struct(shape, &mut |_, _| Ok(()));
    let message = failed.as_ref().map_err(SerdeError::message).err();
    assert_eq!(message, Some("'str' object has no attribute 'tag'"));
    let status = Value::Integer(200);
    let failed = HeaderValue(&status).read_list(shape, &mut |_| Ok(()));
    let message = failed.as_ref().map_err(SerdeError::message).err();
    assert_eq!(message, Some("'int' object is not iterable"));
    Ok(())
}
