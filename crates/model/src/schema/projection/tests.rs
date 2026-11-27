use super::*;
use crate::shapes::Model;
use crate::testing::Result;

const MODEL: &str = r#"{
  "metadata": {"apiVersion": "2020-01-01", "protocol": "json", "jsonVersion": "1.1",
    "protocols": ["json", "query"], "targetPrefix": "T", "serviceId": "S",
    "signingName": "s", "awsQueryCompatible": {}},
  "clientContextParams": {"Flag": {"type": "boolean"}},
  "operations": {"Go": {"name": "GoWire", "http": {"method": "POST", "requestUri": "/"},
    "input": {"shape": "In"}, "output": {"shape": "In", "resultWrapper": "GoResult"},
    "errors": [{"shape": "Oops"}],
    "staticContextParams": {"P": {"value": [1, "a"]}}, "newKey": 1}},
  "shapes": {
    "In": {"type": "structure", "required": ["B", "A"], "members": {
      "B": {"shape": "S", "location": "header", "locationName": "x-b"},
      "A": {"shape": "S", "flattened": false}}},
    "S": {"type": "string", "flattened": true, "min": 1, "max": 2.5, "enum": ["x"]},
    "Oops": {"type": "structure", "members": {}, "exception": true,
      "error": {"code": "Bad", "httpStatusCode": 400, "senderFault": true}},
    "L": {"type": "list", "member": {"shape": "S"}, "event": 1},
    "D": {"type": "structure", "members": {}, "document": true, "union": true}
  }
}"#;

/// The lines, as the frozen digests hold them: a change here is an oracle
/// change.
#[test]
fn a_model_renders_every_read_in_order() -> Result<()> {
    let model = Model::from_json(MODEL.as_bytes())?;
    let expected = [
        concat!(
            r#"service version="2020-01-01" sdk_id="S" endpoint_prefix=- sigv4_name="s" "#,
            r#"auth=- xml_namespace=- query_compatible=true "#,
            r#"protocol_traits=["aws.protocols#awsJson1_1","aws.protocols#awsQuery"]"#
        ),
        concat!(
            r#"botocore protocol="json" protocols=["json","query"] json_version="1.1" "#,
            r#"target_prefix="T" signature_version=- uid=-"#
        ),
        r#"client_context_param "Flag" "boolean""#,
        r#"shape "In" structure "structure" required=["B","A"]"#,
        concat!(
            r#"  member "B" -> "S" flags=flattened location="header" locationName="x-b" "#,
            r#"jsonName="x-b" xmlName="x-b" enum=["x"] min=i1 max=f2.5"#
        ),
        r#"  member "A" -> "S" enum=["x"] min=i1 max=f2.5"#,
        r#"shape "S" string "string" flags=flattened enum=["x"] min=i1 max=f2.5"#,
        r#"shape "Oops" structure "structure" flags=exception error=("Bad",400,true)"#,
        r#"shape "L" list "list" flags=event"#,
        r#"  list_member -> "S" flags=flattened enum=["x"] min=i1 max=f2.5"#,
        r#"shape "D" document "structure" flags=document,union"#,
        concat!(
            r#"operation "Go" wire_name="GoWire" http="POST" "/" host_prefix=- auth=- "#,
            r#"auth_type=- unsigned_payload=false http_checksum_required=false "#,
            r#"endpoint_operation=false http_checksum=- endpoint_discovery=- "#,
            r#"request_compression=- static_context_params=["P"=[1,"a"]] "#,
            r#"operation_context_params=[]"#
        ),
        r#"  input -> "In" required=["B","A"]"#,
        r#"  output -> "In" resultWrapper="GoResult" required=["B","A"]"#,
        r#"  error -> "Oops" flags=exception error=("Bad",400,true)"#,
        r#"uninterpreted operation "Go" "newKey""#,
    ];
    let projection = model.projection();
    assert_eq!(projection.lines().collect::<Vec<_>>(), expected);
    Ok(())
}

/// Each field of a trait view shows in the text, so no slot can differ
/// between two readings unseen.
#[test]
fn every_trait_field_shows() {
    type Edit = fn(&mut TraitsView<'static>);
    let edits: [Edit; 19] = [
        |v| v.flags = vec!["sensitive"],
        |v| v.location = Some("l"),
        |v| v.location_name = Some("l"),
        |v| v.json_name = Some("l"),
        |v| v.xml_name = Some("l"),
        |v| v.ec2_query_name = Some("l"),
        |v| v.payload = Some("l"),
        |v| v.timestamp_format = Some("l"),
        |v| v.xml_namespace = Some((None, None)),
        |v| v.required = vec!["r"],
        |v| v.enum_values = vec!["r"],
        |v| v.min = Some("i1".into()),
        |v| v.max = Some("i1".into()),
        |v| v.pattern = Some("l"),
        |v| v.error = Some((None, None, false)),
        |v| v.retryable = Some(false),
        |v| v.context_param = Some("l"),
        |v| v.result_wrapper = Some("l"),
        |v| v.cold = vec![("a#b", "1".into())],
    ];
    let base = TraitsView::default().text();
    assert_eq!(base, "");
    let texts: std::collections::HashSet<String> = edits
        .iter()
        .map(|edit| {
            let mut view = TraitsView::default();
            edit(&mut view);
            view.text()
        })
        .collect();
    assert_eq!(texts.len(), edits.len());
    assert!(!texts.contains(&base));
}

/// A text with a quote, a space or a newline stays one quoted token, so it
/// cannot forge another field or line.
#[test]
fn texts_are_quoted() {
    let view = TraitsView {
        location_name: Some("a\" b\nshape \"X\""),
        ..TraitsView::default()
    };
    assert_eq!(view.text(), r#" locationName="a\" b\nshape \"X\"""#);
}

/// Bounds keep how the model wrote them.
#[test]
fn bounds_keep_integer_and_float_spelling() {
    assert_eq!(bound(super::super::Bound::Integer(3)), "i3");
    assert_eq!(bound(super::super::Bound::Float(3.0)), "f3.0");
    assert_eq!(bound(super::super::Bound::Float(1e100)), "f1e+100");
}
