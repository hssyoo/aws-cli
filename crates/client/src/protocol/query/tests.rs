use super::*;
use crate::protocol::ParseOptions;
use model::value::Key;

const MODEL: &str = r#"{
  "metadata": {"apiVersion": "2020-01-01", "protocol": "query"},
  "operations": {
    "Go": {"name": "Go", "http": {"method": "POST", "requestUri": "/"},
           "input": {"shape": "In"}, "output": {"shape": "Out", "resultWrapper": "GoResult"},
           "endpoint": {"hostPrefix": "{Name}."}}
  },
  "shapes": {
    "In": {"type": "structure", "members": {
      "Name": {"shape": "S", "hostLabel": true},
      "Tags": {"shape": "List"},
      "On": {"shape": "B"},
      "Text": {"shape": "S"}
    }},
    "Out": {"type": "structure", "members": {"Value": {"shape": "S"}, "Kind": {"shape": "S",
      "xmlAttribute": true, "locationName": "xsi:type"}}},
    "List": {"type": "list", "member": {"shape": "S"}},
    "S": {"type": "string"},
    "B": {"type": "boolean"},
    "Bad": {"type": "structure", "members": {"Detail": {"shape": "S"}}, "error": {"code": "Bad"},
      "exception": true}
  }
}"#;

fn model() -> Result<Model, ParseError> {
    Model::from_json(MODEL.as_bytes()).map_err(|error| ParseError::new(error.to_string()))
}

fn go(model: &Model) -> &Operation {
    &model.operations()[0]
}

fn input(json: &str) -> Value {
    Value::from_json(json.as_bytes()).unwrap_or(Value::Null)
}

/// `QuerySerializer`: `Action` and `Version` first, the caller's order, an
/// empty list as `Name=`, booleans as `true`/`false`, and the host prefix;
/// the body as `AWSRequestPreparer._prepare_body` puts it on the wire,
/// `urlencode(doseq=True)`'s escapes, a space as `+`.
#[test]
fn the_form_is_botocore_s() -> Result<(), Box<dyn std::error::Error>> {
    let model = model()?;
    let request = Query.serialize(
        &model,
        go(&model),
        &input(r#"{"Text":"a b&c=é/~+*'!()","Tags":[],"On":1,"Name":"n"}"#),
    )?;
    let body = String::from_utf8_lossy(&request.body).into_owned();
    assert_eq!(
        body,
        "Action=Go&Version=2020-01-01&Text=a+b%26c%3D%C3%A9%2F~%2B%2A%27%21%28%29\
         &Tags=&On=true&Name=n"
    );
    assert_eq!(request.host_prefix.as_deref(), Some("n."));
    assert_eq!(
        request.headers.get("Content-Type"),
        Some("application/x-www-form-urlencoded; charset=utf-8")
    );
    Ok(())
}

fn response(status: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        headers: crate::http::Headers::new(),
        body: body.as_bytes().to_vec(),
    }
}

/// `_handle_structure`'s `xmlAttribute`, with the namespace renamed to the
/// location name's prefix; `ResponseMetadata` from the body.
#[test]
fn attributes_and_metadata_are_read() -> Result<(), ParseError> {
    let model = model()?;
    let body = concat!(
        "<GoResponse><GoResult><Value xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" ",
        "xsi:type=\"T\">v</Value></GoResult>",
        "<ResponseMetadata><RequestId>id-1</RequestId></ResponseMetadata></GoResponse>"
    );
    let parsed = Query.parse(
        &model,
        go(&model),
        &response(200, body),
        &ParseOptions::BOTOCORE,
    )?;
    let Value::Map(parsed) = parsed else {
        return Err(ParseError::new("not a map"));
    };
    assert_eq!(parsed.get("Value"), Some(&Value::String("v".into())));
    let Some(Value::Map(metadata)) = parsed.get("ResponseMetadata") else {
        return Err(ParseError::new("no metadata"));
    };
    assert_eq!(
        metadata.get("RequestId"),
        Some(&Value::String("id-1".into()))
    );
    assert_eq!(metadata.get("HTTPStatusCode"), Some(&Value::Integer(200)));
    Ok(())
}

/// A query error keeps `Type` in `Error`, after `Code` and `Message` as the
/// body orders them; `RequestId` goes to `ResponseMetadata`; a modelled
/// error's members land at the top level.
#[test]
fn a_query_error_keeps_botocore_s_error_dict() -> Result<(), ParseError> {
    let model = model()?;
    let body = "<ErrorResponse><Error><Type>Sender</Type><Code>Bad</Code><Message>no</Message>\
        <Detail>d</Detail></Error><RequestId>id-2</RequestId></ErrorResponse>";
    let error = Query.parse_error(
        &model,
        go(&model),
        &response(400, body),
        &ParseOptions::BOTOCORE,
    )?;
    let keys: Vec<&str> = error.error.keys().filter_map(Key::as_str).collect();
    assert_eq!(keys, ["Type", "Code", "Message", "Detail"]);
    assert_eq!(
        error.error.get("Type"),
        Some(&Value::String("Sender".into()))
    );
    assert_eq!((error.code(), error.message()), (Some("Bad"), Some("no")));
    assert_eq!(error.request_id.as_deref(), Some("id-2"));
    assert_eq!(
        error.members.get("Detail"),
        Some(&Value::String("d".into()))
    );
    Ok(())
}

/// `ResponseParserError` for a body that is not XML, with expat's message
/// and the body as bytes.
#[test]
fn malformed_xml_is_a_response_parser_error() -> Result<(), ParseError> {
    let model = model()?;
    let error = Query
        .parse(
            &model,
            go(&model),
            &response(200, "<a>"),
            &ParseOptions::BOTOCORE,
        )
        .err();
    let expected = "Unable to parse response (no element found: line 1, column 3), invalid XML \
        received. Further retries may succeed:\nb'<a>'";
    assert_eq!(error.map(|error| error.message).as_deref(), Some(expected));
    Ok(())
}
