use super::*;
use crate::protocol::Protocol;
use model::shapes::Model;
use model::value::Key;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const MODEL: &str = r#"{
  "metadata": {"apiVersion": "2020-01-01", "protocol": "rest-json"},
  "operations": {
    "Go": {"name": "Go", "http": {"method": "GET", "requestUri": "/"},
      "output": {"shape": "Out"}, "errors": [{"shape": "Bad"}]},
    "Blob": {"name": "Blob", "http": {"method": "GET", "requestUri": "/"},
      "output": {"shape": "BlobOut"}},
    "Text": {"name": "Text", "http": {"method": "GET", "requestUri": "/"},
      "output": {"shape": "TextOut"}},
    "Nested": {"name": "Nested", "http": {"method": "GET", "requestUri": "/"},
      "output": {"shape": "NestedOut"}},
    "Doc": {"name": "Doc", "http": {"method": "GET", "requestUri": "/"},
      "output": {"shape": "DocOut"}},
    "Pick": {"name": "Pick", "http": {"method": "GET", "requestUri": "/"},
      "output": {"shape": "PickOut"}},
    "Stream": {"name": "Stream", "http": {"method": "GET", "requestUri": "/"},
      "output": {"shape": "StreamOut"}},
    "None": {"name": "None", "http": {"method": "GET", "requestUri": "/"}}
  },
  "shapes": {
    "Out": {"type": "structure", "members": {
      "Status": {"shape": "I", "location": "statusCode"},
      "Count": {"shape": "I", "location": "header", "locationName": "X-Count"},
      "Ratio": {"shape": "D", "location": "header", "locationName": "X-Ratio"},
      "On": {"shape": "B", "location": "header", "locationName": "X-On"},
      "Names": {"shape": "Names", "location": "header", "locationName": "X-Names"},
      "Counts": {"shape": "Counts", "location": "header", "locationName": "X-Counts"},
      "Json": {"shape": "S", "location": "header", "locationName": "X-Json",
        "jsonvalue": true},
      "Meta": {"shape": "Meta", "location": "headers", "locationName": "X-Meta-"},
      "N": {"shape": "I"}, "F": {"shape": "D"}, "Flag": {"shape": "B"}, "L": {"shape": "Long"},
      "Words": {"shape": "Names"}
    }},
    "BlobOut": {"type": "structure", "payload": "Data", "members": {
      "Data": {"shape": "Data"}}},
    "TextOut": {"type": "structure", "payload": "Text", "members": {
      "Text": {"shape": "S"}}},
    "NestedOut": {"type": "structure", "payload": "Nested", "members": {
      "Nested": {"shape": "Inner"}}},
    "DocOut": {"type": "structure", "payload": "Doc", "members": {
      "Doc": {"shape": "Document"}}},
    "PickOut": {"type": "structure", "payload": "Pick", "members": {
      "Pick": {"shape": "Union"}}},
    "StreamOut": {"type": "structure", "payload": "Events", "members": {
      "Events": {"shape": "Events"}}},
    "Inner": {"type": "structure", "members": {"Name": {"shape": "S"}}},
    "Union": {"type": "structure", "union": true, "members": {"A": {"shape": "S"}}},
    "Document": {"type": "structure", "document": true},
    "Events": {"type": "structure", "eventstream": true, "members": {}},
    "Names": {"type": "list", "member": {"shape": "S"}},
    "Counts": {"type": "list", "member": {"shape": "I"}},
    "Meta": {"type": "map", "key": {"shape": "S"}, "value": {"shape": "S"}},
    "Bad": {"type": "structure", "exception": true, "error": {"httpStatusCode": 400},
      "members": {
        "Reason": {"shape": "S", "location": "header", "locationName": "X-Reason"},
        "Status": {"shape": "I", "location": "statusCode"},
        "Detail": {"shape": "S"}
      }},
    "S": {"type": "string"}, "I": {"type": "integer"}, "Long": {"type": "long"},
    "D": {"type": "double"}, "B": {"type": "boolean"}, "Data": {"type": "blob"}
  }
}"#;

fn model() -> Result<Model, Box<dyn std::error::Error>> {
    Ok(Model::from_json(MODEL.as_bytes())?)
}

fn response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> HttpResponse {
    HttpResponse {
        status,
        headers: Headers::from_pairs(headers.iter().copied()),
        body: body.to_vec(),
    }
}

/// `operation`'s output from a 200 response, `ResponseMetadata` aside.
fn output(operation: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Map, ParseError> {
    let model = Model::from_json(MODEL.as_bytes()).map_err(|e| ParseError::new(e.to_string()))?;
    let operation = model
        .schema()
        .service()
        .operation(operation)
        .ok_or_else(|| ParseError::new("operation"))?;
    match RestJson.parse(
        &model,
        operation,
        &response(200, headers, body),
        &ParseOptions::BOTOCORE,
    )? {
        Value::Map(mut parsed) => {
            parsed.remove("ResponseMetadata");
            Ok(parsed)
        }
        other => Err(ParseError::new(format!("not a map: {other:?}"))),
    }
}

/// The error a `status` response to `Go` parses to.
fn error(status: u16, headers: &[(&str, &str)], body: &str) -> Result<ServiceError, ParseError> {
    let model = Model::from_json(MODEL.as_bytes()).map_err(|e| ParseError::new(e.to_string()))?;
    let operation = model
        .schema()
        .service()
        .operation("Go")
        .ok_or_else(|| ParseError::new("operation"))?;
    RestJson.parse_error(
        &model,
        operation,
        &response(status, headers, body.as_bytes()),
        &ParseOptions::BOTOCORE,
    )
}

use crate::error::ServiceError;

fn text(value: &str) -> Value {
    Value::String(value.into())
}

fn json(text: &str) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(Value::from_json(text.as_bytes())?)
}

/// Response headers, a body, and the code they give.
type CodeCase<'a> = (&'a [(&'a str, &'a str)], &'a str, &'a str);

/// `_inject_error_code`: `x-amzn-errortype` first, then the body's `code`,
/// then `Code`; only then `BaseJSONParser`'s `x-amzn-query-error` and
/// `__type`, and last the status. Each text code loses what follows a `:`
/// and what precedes the last `#`.
#[test]
fn the_error_code_comes_from_the_header_then_code_then_type() -> TestResult {
    let everything = r#"{"code":"aws#FromCode","Code":"FromUpper","__type":"FromType"}"#;
    let cases: [CodeCase<'_>; 8] = [
        (
            &[("X-Amzn-Errortype", "ns#FromHeader:http://internal/")],
            everything,
            "FromHeader",
        ),
        (&[], everything, "FromCode"),
        (&[], r#"{"Code":"a.b#FromUpper","__type":"T"}"#, "FromUpper"),
        (
            &[("x-amzn-query-error", "Query;Sender")],
            r#"{"__type":"T"}"#,
            "Query",
        ),
        (
            &[],
            r#"{"__type":"aws.protocoltests#FromType:uri"}"#,
            "FromType",
        ),
        (&[], "{}", "400"),
        // A `null` code is no code: `__type`'s stands.
        (&[], r#"{"code":null,"Code":"Upper","__type":"T"}"#, "T"),
        // An empty header is a code all the same.
        (&[("x-amzn-errortype", "")], everything, ""),
    ];
    for (headers, body, code) in cases {
        let parsed = error(400, headers, body)?;
        assert_eq!(parsed.code(), Some(code), "{headers:?} {body}");
    }
    let query = error(
        400,
        &[
            ("x-amzn-errortype", "Rest"),
            ("x-amzn-query-error", "Query;Sender"),
        ],
        "{}",
    )?;
    assert_eq!(query.code(), Some("Rest"));
    assert_eq!(query.error.get("Type"), Some(&text("Sender")));
    assert_eq!(
        query.error.keys().collect::<Vec<_>>(),
        ["Message", "Code", "Type"]
    );
    let number = error(400, &[], r#"{"code":5,"message":"m"}"#)?;
    assert_eq!(number.error.get("Code"), Some(&Value::Integer(5)));
    assert_eq!(number.message(), Some("m"));
    Ok(())
}

/// The error's `ResponseMetadata` holds `x-amzn-requestid` alone; a body
/// that is not JSON is the message; one that is no object fails as
/// `body.get` does.
#[test]
fn error_bodies_parse_as_base_json_parser_s() -> TestResult {
    let parsed = error(
        400,
        &[("x-amzn-requestid", "r"), ("x-amz-request-id", "s")],
        "not json",
    )?;
    assert_eq!(
        (parsed.code(), parsed.message()),
        (Some("400"), Some("not json"))
    );
    assert_eq!(parsed.request_id.as_deref(), Some("r"));
    let other = error(400, &[("x-amz-request-id", "s")], "{}")?;
    assert_eq!(other.request_id, None);
    for (body, message) in [
        ("[1]", "'list' object has no attribute 'get'"),
        ("null", "'NoneType' object has no attribute 'get'"),
        ("\"text\"", "'str' object has no attribute 'get'"),
    ] {
        assert_eq!(
            error(400, &[], body),
            Err(ParseError::new(message)),
            "{body}"
        );
    }
    let model = model()?;
    let operation = model
        .schema()
        .service()
        .operation("Go")
        .ok_or("operation")?;
    let latin = response(400, &[], b"{\"message\":\"\xe9\"}");
    assert_eq!(
        RestJson.parse_error(&model, operation, &latin, &ParseOptions::BOTOCORE),
        Err(ParseError::new(
            "'utf-8' codec can't decode byte 0xe9 in position 12: invalid continuation byte"
        ))
    );
    Ok(())
}

/// A modelled error's second parse reads its header and status members as
/// well as its body's, into `ServiceError.members`.
#[test]
fn a_modelled_error_s_members_include_its_headers_and_status() -> TestResult {
    let parsed = error(
        409,
        &[
            ("x-amzn-errortype", "Bad"),
            ("X-Reason", "busy"),
            ("x-amzn-requestid", "r"),
        ],
        r#"{"message":"m","Detail":"d","Other":1}"#,
    )?;
    assert_eq!((parsed.code(), parsed.message()), (Some("Bad"), Some("m")));
    assert_eq!(parsed.status, 409);
    assert_eq!(parsed.request_id.as_deref(), Some("r"));
    assert_eq!(parsed.error.keys().collect::<Vec<_>>(), ["Message", "Code"]);
    let mut members = Map::new();
    members.insert("Reason".into(), text("busy"));
    members.insert("Status".into(), Value::Integer(409));
    members.insert("Detail".into(), text("d"));
    assert_eq!(parsed.members, members);
    Ok(())
}

/// `_handle_list` splits a header's text at every comma, quotes or not,
/// and strips each part; each part then parses as the list's member.
#[test]
fn header_lists_split_at_every_comma() -> TestResult {
    let parsed = output(
        "Go",
        &[("X-Names", " a,\"b, c\" ,,\u{1f}d\t"), ("X-Counts", "1, 2")],
        b"",
    )?;
    let names = ["a", "\"b", "c\"", "", "d"].map(text).to_vec();
    assert_eq!(parsed.get("Names"), Some(&Value::List(names)));
    let counts = vec![Value::Integer(1), Value::Integer(2)];
    assert_eq!(parsed.get("Counts"), Some(&Value::List(counts)));
    // A body list is a list already, its items not split.
    let body = output("Go", &[], br#"{"Words":["a,b"]}"#)?;
    assert_eq!(body.get("Words"), Some(&Value::List(vec![text("a,b")])));
    Ok(())
}

/// A `jsonvalue` header is base64 JSON; a header absent stays absent.
#[test]
fn json_value_headers_are_base64_json() -> TestResult {
    let parsed = output("Go", &[("x-json", "eyJhIjogWzEsIHRydWVdfQ==")], b"")?;
    assert_eq!(parsed.get("Json"), Some(&json(r#"{"a": [1, true]}"#)?));
    assert_eq!(parsed.get("Count"), None);
    for (value, message) in [
        ("e30", "Incorrect padding"),
        (
            "\u{e9}",
            "string argument should contain only ASCII characters",
        ),
        (
            "/w==",
            "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte",
        ),
        // `json.loads`' own errors, with CPython's text.
        ("bm9wZQ==", "Expecting value: line 1 column 1 (char 0)"),
        (
            "eyJhIjogMSx9",
            "Illegal trailing comma before end of object: line 1 column 8 (char 7)",
        ),
    ] {
        assert_eq!(
            output("Go", &[("X-Json", value)], b""),
            Err(ParseError::new(message)),
            "{value}"
        );
    }
    Ok(())
}

/// Header scalars through `RestJSONParser`'s handlers: `int()`, `float()`,
/// `ensure_boolean`; the status member from the status; a repeated header
/// joined with `, `.
#[test]
fn header_scalars_parse_as_python_converts_them() -> TestResult {
    let headers = [
        ("X-Count", " 12 "),
        ("X-Ratio", "-Infinity"),
        ("X-On", "TRUE"),
        ("X-Names", "a"),
        ("x-names", "b"),
    ];
    let parsed = output("Go", &headers, b"")?;
    assert_eq!(parsed.get("Status"), Some(&Value::Integer(200)));
    assert_eq!(parsed.get("Count"), Some(&Value::Integer(12)));
    assert_eq!(parsed.get("Ratio"), Some(&Value::Float(f64::NEG_INFINITY)));
    // int() reads any Unicode decimal digits, as CPython does.
    let arabic = output("Go", &[("X-Count", "\u{661}\u{662}")], b"")?;
    assert_eq!(arabic.get("Count"), Some(&Value::Integer(12)));
    assert_eq!(parsed.get("On"), Some(&Value::Bool(true)));
    assert_eq!(
        parsed.get("Names"),
        Some(&Value::List(vec![text("a"), text("b")]))
    );
    assert_eq!(
        output("Go", &[("X-On", "1")], b"")?.get("On"),
        Some(&Value::Bool(false))
    );
    for (header, value, message) in [
        (
            "X-Count",
            "1.5",
            "invalid literal for int() with base 10: '1.5'",
        ),
        ("X-Count", "", "invalid literal for int() with base 10: ''"),
        // C's six are stripped, not str.strip()'s separators.
        (
            "X-Count",
            "\u{1c}7",
            "invalid literal for int() with base 10: '\\x1c7'",
        ),
        ("X-Ratio", "abc", "could not convert string to float: 'abc'"),
    ] {
        assert_eq!(
            output("Go", &[(header, value)], b""),
            Err(ParseError::new(message)),
            "{header}: {value}"
        );
    }
    Ok(())
}

/// The same handlers on the body: an int float is a float, a float int is
/// truncated, a number is no boolean.
#[test]
fn body_scalars_parse_as_python_converts_them() -> TestResult {
    let parsed = output(
        "Go",
        &[],
        br#"{"N": -1.9, "F": 1, "Flag": 1, "L": 1e20, "Status": "201"}"#,
    )?;
    // A member outside the body is read from the body too, and the body's
    // value wins in its place.
    let expected = r#"{"Status": 201, "Meta": {}, "N": -1, "F": 1.0, "Flag": false,
        "L": 100000000000000000000}"#;
    assert_eq!(Value::Map(parsed), json(expected)?);
    let strings = output("Go", &[], br#"{"N": "7", "F": " 2.5 ", "Flag": "True"}"#)?;
    let expected = r#"{"Status": 200, "Meta": {}, "N": 7, "F": 2.5, "Flag": true}"#;
    assert_eq!(Value::Map(strings), json(expected)?);
    Ok(())
}

/// Each conversion's error, as Python words it.
#[test]
fn body_scalars_fail_as_python_conversions_do() {
    for (body, message) in [
        (
            r#"{"N": [1]}"#,
            "int() argument must be a string, a bytes-like object or a real number, not 'list'",
        ),
        (
            r#"{"F": {}}"#,
            "float() argument must be a string or a real number, not 'dict'",
        ),
        (
            r#"{"N": 1e400}"#,
            "cannot convert float infinity to integer",
        ),
        (
            &format!(r#"{{"F": 1{}}}"#, "0".repeat(400)),
            "int too large to convert to float",
        ),
    ] {
        assert_eq!(
            output("Go", &[], body.as_bytes()),
            Err(ParseError::new(message)),
            "{body}"
        );
    }
}

/// `_parse_header_map`: a prefix in any case, keys as the header came
/// without it, values as they are.
#[test]
fn header_maps_take_every_header_with_the_prefix() -> TestResult {
    let parsed = output(
        "Go",
        &[
            ("x-meta-One", "1"),
            ("X-META-two", "2"),
            ("x-meta-one", "3"),
            ("Other", "4"),
        ],
        b"",
    )?;
    let mut meta = Map::new();
    meta.insert("One".into(), text("1, 3"));
    meta.insert("two".into(), text("2"));
    assert_eq!(parsed.get("Meta"), Some(&Value::Map(meta)));
    let empty = output("Go", &[], b"")?;
    assert_eq!(empty.get("Meta"), Some(&Value::Map(Map::new())));
    Ok(())
}

/// `_populate_response_metadata`: `x-amzn-requestid`, else
/// `x-amz-request-id` with `HostId` from `x-amz-id-2` or `''`.
#[test]
fn the_request_id_comes_from_either_header() -> TestResult {
    let model = model()?;
    let operation = model
        .schema()
        .service()
        .operation("None")
        .ok_or("operation")?;
    for (headers, metadata) in [
        (
            &[("x-amzn-requestid", "a"), ("x-amz-request-id", "b")][..],
            r#"{"RequestId": "a"}"#,
        ),
        (
            &[("x-amz-request-id", "b")],
            r#"{"RequestId": "b", "HostId": ""}"#,
        ),
        (
            &[("x-amz-request-id", "b"), ("x-amz-id-2", "h")],
            r#"{"RequestId": "b", "HostId": "h"}"#,
        ),
        (&[], "{}"),
    ] {
        let parsed = RestJson.do_parse(
            operation.output(),
            &response(200, headers, b"not json"),
            &ParseOptions::BOTOCORE,
        )?;
        let mut expected = Map::new();
        expected.insert("ResponseMetadata".into(), json(metadata)?);
        assert_eq!(parsed, Value::Map(expected), "{headers:?}");
    }
    Ok(())
}

/// An empty body is `{}`; a body not JSON is `{'message': body}`, which no
/// member reads; a `null` body is `None`, which `final_parsed.update`
/// refuses.
#[test]
fn empty_and_null_bodies_are_botocore_s() -> TestResult {
    assert_eq!(
        output("Go", &[], b"")?.keys().collect::<Vec<_>>(),
        ["Status", "Meta"]
    );
    assert_eq!(output("Go", &[], b"<html>")?.len(), 2);
    assert_eq!(
        output("Go", &[], b"null"),
        Err(ParseError::new("'NoneType' object is not iterable"))
    );
    assert_eq!(
        output("Nested", &[], b"null")?.get("Nested"),
        Some(&Value::Null)
    );
    assert_eq!(
        output("Nested", &[], b"")?.get("Nested"),
        Some(&Value::Map(Map::new()))
    );
    Ok(())
}

/// `_parse_payload`: a blob or string payload is the decoded body, an empty
/// one `''`.
#[test]
fn text_payloads_are_the_decoded_body() -> TestResult {
    assert_eq!(
        output("Blob", &[], b"hi\n")?.get("Data"),
        Some(&text("hi\n"))
    );
    assert_eq!(output("Blob", &[], b"")?.get("Data"), Some(&text("")));
    assert_eq!(
        output("Text", &[], b"{\"a\":1}")?.get("Text"),
        Some(&text("{\"a\":1}"))
    );
    assert_eq!(
        output("Text", &[], b"\xe4\xb8"),
        Err(ParseError::new(
            "'utf-8' codec can't decode bytes in position 0-1: unexpected end of data"
        ))
    );
    Ok(())
}

/// A structure, document or union payload is the JSON body, an event
/// stream refused.
#[test]
fn json_payloads_are_the_parsed_body() -> TestResult {
    let nested = output("Nested", &[], br#"{"Name":"n","Other":1}"#)?;
    assert_eq!(nested.get("Nested"), Some(&json(r#"{"Name":"n"}"#)?));
    assert_eq!(
        output("Doc", &[], b"\"text\"")?.get("Doc"),
        Some(&text("text"))
    );
    assert_eq!(output("Doc", &[], b"[1]")?.get("Doc"), Some(&json("[1]")?));
    assert_eq!(
        output("Pick", &[], br#"{"A":"a"}"#)?.get("Pick"),
        Some(&json(r#"{"A":"a"}"#)?)
    );
    assert_eq!(
        output("Pick", &[], b""),
        Err(ParseError::new(
            "Invalid service response: Union must have one and only one member set."
        ))
    );
    assert_eq!(
        output("Stream", &[], b""),
        Err(ParseError::new("event stream responses are not supported"))
    );
    Ok(())
}

/// A gzip body, which botocore leaves compressed (`URLLib3Session.send`'s
/// `decode_content=False`), fails as its `bytes.decode` does.
#[test]
fn a_gzip_body_fails_as_its_decode_does() {
    assert_eq!(
        output("Go", &[("Content-Encoding", "gzip")], b"\x1f\x8b\x08\x00"),
        Err(ParseError::new(
            "'utf-8' codec can't decode byte 0x8b in position 1: invalid start byte"
        ))
    );
}

/// `BaseRestParser._do_parse` sets `ResponseMetadata` before the output's
/// members, and `ResponseParser.parse` then adds the status and headers to
/// that same dict, which keeps its place.
#[test]
fn response_metadata_comes_before_the_output_s_members() -> TestResult {
    let model = model()?;
    let operation = model.schema().service().operation("Go").ok_or("Go")?;
    let parsed = RestJson.parse(
        &model,
        operation,
        &response(200, &[("X-Count", "3")], br#"{"N": 1}"#),
        &ParseOptions::BOTOCORE,
    )?;
    let Value::Map(parsed) = parsed else {
        return Err("not a map".into());
    };
    let keys: Vec<&str> = parsed.keys().filter_map(Key::as_str).collect();
    assert_eq!(keys.first(), Some(&"ResponseMetadata"), "{keys:?}");
    let metadata = parsed
        .get("ResponseMetadata")
        .and_then(Value::as_map)
        .ok_or("metadata")?;
    assert_eq!(metadata.get("HTTPStatusCode"), Some(&Value::Integer(200)));
    Ok(())
}

/// A document payload is `json.loads`' document: `NaN`, the infinities and
/// a lone surrogate held; nesting past the reader's limit fails the call,
/// naming the native client's limit.
#[test]
fn a_document_payload_loads_as_python_s_json_loads() -> TestResult {
    let parsed = output("Doc", &[], b"[NaN, -Infinity]")?;
    let Some(Value::List(doc)) = parsed.get("Doc") else {
        return Err("Doc".into());
    };
    assert!(matches!(doc[0], Value::Float(number) if number.is_nan()));
    assert_eq!(doc[1], Value::Float(f64::NEG_INFINITY));
    let lone = output("Doc", &[], br#"["\udfff"]"#)?;
    assert_eq!(
        lone.get("Doc"),
        Some(&Value::List(vec![Value::from_text(
            model::value::Wtf8Buf::from_code_points([0xDFFF])
        )]))
    );
    let deep = |depth| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    let failed = output("Doc", &[], deep(model::pyjson::MAX_DEPTH + 1).as_bytes()).err();
    assert!(
        failed
            .as_ref()
            .is_some_and(|error| error.message.starts_with("native client limit: ")),
        "{failed:?}"
    );
    let at_limit = deep(model::pyjson::MAX_DEPTH);
    std::thread::spawn(move || output("Doc", &[], at_limit.as_bytes()).map(|_| ()))
        .join()
        .map_err(|_| "the parse overflowed its stack")??;
    Ok(())
}
