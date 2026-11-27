//! rpcv2Cbor is built but switched off; and what the protocol suite
//! leaves open: the headers' order, the error step's codes and hooks.
use super::*;
use crate::protocol::{IMPLEMENTED, SWITCHED_OFF, select};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const MODEL: &str = r#"{
  "metadata": {"apiVersion": "2020-01-01", "protocol": "smithy-rpc-v2-cbor",
    "protocols": ["smithy-rpc-v2-cbor"], "targetPrefix": "Svc", "awsQueryCompatible": {}},
  "operations": {
    "Bare": {"name": "Bare", "http": {"method": "GET", "requestUri": "/x"}},
    "Body": {"name": "Body", "http": {"method": "POST", "requestUri": "/"},
      "input": {"shape": "In"}, "output": {"shape": "Out"}}},
  "shapes": {
    "In": {"type": "structure", "members": {"A": {"shape": "S"}}},
    "Out": {"type": "structure", "members": {"A": {"shape": "S"}, "W": {"shape": "W"}}},
    "S": {"type": "string"}, "W": {"type": "timestamp"}
  }
}"#;

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap_or(0))
        .collect()
}

fn response(status: u16, headers: &[(&str, &str)], body: &str) -> HttpResponse {
    HttpResponse {
        status,
        headers: Headers::from_pairs(headers.iter().copied()),
        body: hex(body),
    }
}

#[test]
fn rpc_v2_cbor_is_built_but_not_admitted() -> Result<()> {
    let model = Model::from_json(MODEL.as_bytes())?;
    assert!(select(&model).is_err());
    assert!(IMPLEMENTED.named("smithy-rpc-v2-cbor").is_none());
    let named = SWITCHED_OFF.named("smithy-rpc-v2-cbor");
    assert_eq!(named.map(|p| p.protocol_id()), Some(traits::RPC_V2_CBOR));
    Ok(())
}

/// `POST` whatever the model says, the path from `targetPrefix`, and the
/// headers in botocore's order; no body and no `Content-Type` without an
/// input.
#[test]
fn requests_are_botocore_s() -> Result<()> {
    let model = Model::from_json(MODEL.as_bytes())?;
    let service = model.schema().service();
    let bare = service.operation("Bare").ok_or("Bare")?;
    let request = RPC_V2_CBOR.serialize_request(bare, &Value::Map(Map::new()), &model)?;
    assert_eq!(request.method, Method::Post);
    assert_eq!(request.path, "/service/Svc/operation/Bare");
    assert!(request.body.is_empty());
    let names: Vec<&str> = request.headers.iter().map(|(name, _)| name).collect();
    assert_eq!(names, ["smithy-protocol", "x-amzn-query-mode", "Accept"]);
    let body = service.operation("Body").ok_or("Body")?;
    let request =
        RPC_V2_CBOR.serialize_request(body, &Value::from_json(br#"{"A": "a"}"#)?, &model)?;
    assert_eq!(request.body, hex("a161416161"));
    assert_eq!(
        request.headers.get("Content-Type"),
        Some("application/cbor")
    );
    let unprefixed = Model::from_json(MODEL.replace(r#""targetPrefix": "Svc","#, "").as_bytes())?;
    let refused = RPC_V2_CBOR.serialize_request(bare, &Value::Map(Map::new()), &unprefixed);
    assert_eq!(
        refused.err().map(|e| e.message),
        Some("'targetPrefix'".into())
    );
    Ok(())
}

/// The error code as `_do_error_parse` cuts it, the query-compatible header
/// over it with the body's kept, and Python's errors for a code that is
/// no `str` and a body that is no map.
#[test]
fn the_error_step_reads_codes_as_botocore_does() -> Result<()> {
    let qc = [
        ("x-amzn-query-error", "Q.Code;Sender"),
        ("x-amzn-requestid", "r"),
    ];
    // {"__type": "a.b#Missing:extra", "Message": "m"}
    let parsed = RPC_V2_CBOR.parse_error_metadata(&response(
        400,
        &qc,
        "a2665f5f7479706571612e62234d697373696e673a6578747261674d657373616765616d",
    ))?;
    let expected = Value::from_json(
        br#"{"Error": {"Message": "m", "Code": "Q.Code", "QueryErrorCode": "Missing",
             "Type": "Sender"}, "ResponseMetadata": {"RequestId": "r"}}"#,
    )?;
    assert_eq!(Value::Map(parsed), expected);
    for (body, error) in [
        ("", "'bytes' object has no attribute 'get'"),
        ("80", "'list' object has no attribute 'get'"),
        (
            "a1665f5f747970650c",
            "argument of type 'int' is not iterable",
        ),
        (
            "a1665f5f7479706581613a",
            "'list' object has no attribute 'split'",
        ),
    ] {
        let refused = RPC_V2_CBOR.parse_error_metadata(&response(400, &[], body));
        assert_eq!(
            refused.err().map(|e| e.message),
            Some(error.to_owned()),
            "{body}"
        );
    }
    // No code: the status.
    let parsed = RPC_V2_CBOR.parse_error_metadata(&response(404, &[], "a0"))?;
    let code = parsed.get("Error").and_then(|error| error.get("Code"));
    assert_eq!(code, Some(&Value::String("404".into())));
    Ok(())
}

/// A call's timestamp parser reaches tag 1 values in an error body as in an
/// output, and a member is read by its member name alone.
#[test]
fn the_call_s_hooks_reach_every_tag() -> Result<()> {
    let model = Model::from_json(MODEL.as_bytes())?;
    let body = model.schema().service().operation("Body").ok_or("Body")?;
    let refuse = |_: &Value| Err(ParseError::new("hook"));
    let options = ParseOptions {
        timestamp: &refuse,
        blob: &refuse,
    };
    // {"__type": "E", "W": 1(0)}
    let error = response(400, &[], "a2665f5f7479706561456157c100");
    let registry = model.schema().registry();
    let refused = RPC_V2_CBOR.parse_error(body, &error, &options, registry);
    assert_eq!(refused.err().map(|e| e.message), Some("hook".into()));
    let keep = |value: &Value| Ok(value.clone());
    let options = ParseOptions {
        timestamp: &keep,
        blob: &keep,
    };
    let output = RPC_V2_CBOR.parse(body, &response(200, &[], "a2614161786157c105"), &options)?;
    let expected = Value::from_json(
        br#"{"A": "x", "W": 5, "ResponseMetadata": {"HTTPStatusCode": 200,
        "HTTPHeaders": {}}}"#,
    )?;
    assert_eq!(output, expected);
    Ok(())
}
