//! rest-xml is admitted: admission and clients select it; and the request
//! quirks the protocol suite leaves open.
use super::*;
use crate::protocol::{IMPLEMENTED, SWITCHED_OFF, select};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const MODEL: &str = r#"{
  "metadata": {"apiVersion": "2020-01-01", "protocol": "rest-xml", "endpointPrefix": "svc",
    "protocols": ["rest-xml"]},
  "operations": {
    "Bare": {"name": "Bare", "http": {"method": "DELETE", "requestUri": "/b/{Id}?x"}},
    "Body": {"name": "Body", "http": {"method": "POST", "requestUri": "/p"},
      "input": {"shape": "In"}}},
  "shapes": {
    "In": {"type": "structure", "members": {"A": {"shape": "S"},
      "H": {"shape": "S", "location": "header", "locationName": "x-h"}}},
    "S": {"type": "string"}
  }
}"#;

#[test]
fn rest_xml_is_admitted() -> Result<()> {
    let model = Model::from_json(MODEL.as_bytes())?;
    let selected = select(&model).map_err(|unsupported| format!("{unsupported:?}"))?;
    assert_eq!(selected.protocol_id(), traits::REST_XML);
    let named = IMPLEMENTED
        .named("rest-xml")
        .map(|protocol| protocol.protocol_id());
    assert_eq!(named, Some(traits::REST_XML));
    assert!(IMPLEMENTED.get(traits::REST_XML).is_some());
    assert!(SWITCHED_OFF.named("rest-xml").is_none());
    Ok(())
}

/// No input: the template as it is, labels and all. A body without the
/// input's `locationName` is `KeyError: 'name'`; with only located members
/// set there is no body, and never a `Content-Type`.
#[test]
fn requests_without_a_body_or_a_root_name() -> Result<()> {
    let model = Model::from_json(MODEL.as_bytes())?;
    let service = model.schema().service();
    let bare = service.operation("Bare").ok_or("Bare")?;
    let request = REST_XML.serialize_request(bare, &Value::Map(Map::new()), &model)?;
    assert_eq!(
        (request.path.as_str(), request.query.as_str()),
        ("/b/{Id}", "x")
    );
    let body = service.operation("Body").ok_or("Body")?;
    let input = Value::from_json(br#"{"H": "h"}"#)?;
    let request = REST_XML.serialize_request(body, &input, &model)?;
    assert!(request.body.is_empty());
    assert_eq!(request.headers.get("x-h"), Some("h"));
    assert!(!request.headers.contains("Content-Type"));
    let input = Value::from_json(br#"{"A": "a"}"#)?;
    let refused = REST_XML.serialize_request(body, &input, &model);
    assert_eq!(
        refused.err().map(|error| error.message),
        Some("'name'".to_owned())
    );
    Ok(())
}
