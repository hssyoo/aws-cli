use super::*;
use crate::protocol::ParseOptions;
use model::value::Key;

const MODEL: &str = r#"{
  "metadata": {"apiVersion": "2020-01-01", "protocol": "json", "jsonVersion": "1.0",
    "targetPrefix": "Svc", "awsQueryCompatible": {}},
  "operations": {"Go": {"name": "Go", "http": {"method": "POST", "requestUri": "/"},
    "input": {"shape": "In"}, "output": {"shape": "Out"}}},
  "shapes": {
    "In": {"type": "structure", "members": {"When": {"shape": "T"}, "Data": {"shape": "Blob"},
      "N": {"shape": "D"}}},
    "Out": {"type": "structure", "members": {"When": {"shape": "T"}, "Data": {"shape": "Blob"},
      "Pick": {"shape": "U"}, "Doc": {"shape": "Document"}}},
    "Document": {"type": "structure", "members": {}, "document": true},
    "U": {"type": "structure", "union": true, "members": {"A": {"shape": "S"}}},
    "T": {"type": "timestamp"}, "Blob": {"type": "blob"}, "D": {"type": "double"},
      "S": {"type": "string"},
    "Bad": {"type": "structure", "members": {"Detail": {"shape": "S"}}, "exception": true}
  }
}"#;

fn model() -> Option<Model> {
    Model::from_json(MODEL.as_bytes()).ok()
}

fn response(status: u16, headers: &[(&str, &str)], body: &str) -> HttpResponse {
    HttpResponse {
        status,
        headers: Headers::from_pairs(headers.iter().copied()),
        body: body.as_bytes().to_vec(),
    }
}

/// `JSONSerializer`: the target, the version's content type, the
/// query-compatible header, epoch timestamps, base64 blobs, special floats.
#[test]
fn the_request_is_botocore_s() -> Result<(), Box<dyn std::error::Error>> {
    let model = model().ok_or("model")?;
    let operation = &model.operations()[0];
    let input = Value::from_json(br#"{"N":1.5,"When":"2015-08-30T12:36:00Z","Data":"hi"}"#)?;
    let request = Json.serialize(&model, operation, &input)?;
    assert_eq!(request.headers.get("X-Amz-Target"), Some("Svc.Go"));
    assert_eq!(
        request.headers.get("Content-Type"),
        Some("application/x-amz-json-1.0")
    );
    assert_eq!(request.headers.get("x-amzn-query-mode"), Some("true"));
    assert_eq!(
        String::from_utf8(request.body)?,
        r#"{"N": 1.5, "When": 1440938160, "Data": "aGk="}"#
    );
    let empty = Json.serialize(&model, operation, &Value::Map(Map::new()))?;
    assert_eq!(empty.body, b"{}");
    Ok(())
}

/// A query-compatible error: `x-amzn-query-error` gives `Code` and `Type`;
/// `Message`, `Code`, `Type` in botocore's insertion order.
#[test]
fn a_query_compatible_error_takes_its_code_from_the_header()
-> Result<(), Box<dyn std::error::Error>> {
    let model = model().ok_or("model")?;
    let headers = [
        ("x-amzn-query-error", "AWS.Old;Sender"),
        ("x-amzn-requestid", "r"),
    ];
    let body = r#"{"__type":"com.amazon#New","message":"m"}"#;
    let error = Json.parse_error(
        &model,
        &model.operations()[0],
        &response(400, &headers, body),
        &ParseOptions::BOTOCORE,
    )?;
    assert_eq!(
        error.error.keys().collect::<Vec<_>>(),
        ["Message", "Code", "Type"]
    );
    assert_eq!(
        (error.code(), error.message()),
        (Some("AWS.Old"), Some("m"))
    );
    assert_eq!(
        error.error.get("Type"),
        Some(&Value::String("Sender".into()))
    );
    assert_eq!(error.request_id.as_deref(), Some("r"));
    Ok(())
}

/// `__type` without its namespace or URI suffix; keys the error parse does
/// not read stay out of `Error`; a modelled error's members land at the top.
#[test]
fn json_error_codes_and_modelled_members_are_botocore_s() -> Result<(), Box<dyn std::error::Error>>
{
    let model = model().ok_or("model")?;
    let operation = &model.operations()[0];
    for (body, code) in [
        (
            r#"{"__type":"aws.protocoltests#Bad:http://internal/","Detail":"d","Other":1}"#,
            "Bad",
        ),
        (r#"{"__type":"Plain"}"#, "Plain"),
        (r#"{}"#, "500"),
    ] {
        let error = Json.parse_error(
            &model,
            operation,
            &response(500, &[], body),
            &ParseOptions::BOTOCORE,
        )?;
        assert_eq!(error.code(), Some(code), "{body}");
        assert_eq!(error.error.keys().collect::<Vec<_>>(), ["Message", "Code"]);
        let members: Vec<&str> = error.members.keys().filter_map(Key::as_str).collect();
        let expected: &[&str] = if code == "Bad" { &["Detail"] } else { &[] };
        assert_eq!(members, expected, "{body}");
    }
    let plain = Json.parse_error(
        &model,
        operation,
        &response(400, &[], "not json"),
        &ParseOptions::BOTOCORE,
    )?;
    assert_eq!(
        (plain.code(), plain.message()),
        (Some("400"), Some("not json"))
    );
    Ok(())
}

/// The CLI's options: timestamps and blobs as the wire gave them.
#[test]
fn the_cli_s_handlers_keep_the_wire_values() -> Result<(), Box<dyn std::error::Error>> {
    let model = model().ok_or("model")?;
    let wire = |value: &Value| Ok(value.clone());
    let text = |text: &str| Ok(Value::String(text.to_owned()));
    let options = ParseOptions {
        timestamp: &wire,
        blob: &text,
    };
    let body = r#"{"When":1440938160.5,"Data":"aGk=","Pick":{"New":1}}"#;
    let Value::Map(parsed) = Json.parse(
        &model,
        &model.operations()[0],
        &response(200, &[], body),
        &options,
    )?
    else {
        return Err("not a map".into());
    };
    assert_eq!(parsed.get("When"), Some(&Value::Float(1_440_938_160.5)));
    assert_eq!(parsed.get("Data"), Some(&Value::String("aGk=".into())));
    let unknown = Value::from_json(br#"{"SDK_UNKNOWN_MEMBER":{"name":"New"}}"#)?;
    assert_eq!(parsed.get("Pick"), Some(&unknown));
    Ok(())
}

/// The output of `Go` for `body`, or the parser's error.
fn output(headers: &[(&str, &str)], body: &[u8]) -> Result<Value, ParseError> {
    let model = model().ok_or_else(|| ParseError::new("model"))?;
    let operation = &model.operations()[0];
    let response = HttpResponse {
        status: 200,
        headers: Headers::from_pairs(headers.iter().copied()),
        body: body.to_vec(),
    };
    Json.parse(&model, operation, &response, &ParseOptions::BOTOCORE)
}

/// A body is `json.loads`' document: `NaN` and the infinities are floats,
/// which serde's reader refused, so the member was dropped.
#[test]
fn a_body_holds_nan_and_the_infinities() -> Result<(), Box<dyn std::error::Error>> {
    let parsed = output(&[], br#"{"Doc": [NaN, Infinity, -Infinity, 1e400]}"#)?;
    let Some(Value::List(doc)) = parsed.get("Doc") else {
        return Err("Doc".into());
    };
    assert!(matches!(doc[0], Value::Float(number) if number.is_nan()));
    assert_eq!(
        doc[1..],
        [
            Value::Float(f64::INFINITY),
            Value::Float(f64::NEG_INFINITY),
            Value::Float(f64::INFINITY)
        ]
    );
    Ok(())
}

/// What the reader cannot hold fails the call, naming the native client's
/// limit, rather than reading as `{'message': body}` and printing nothing:
/// a lone surrogate, and nesting past the limit.
#[test]
fn a_body_past_the_reader_s_limits_fails_naming_the_limit() {
    let nested = |depth| format!("{{\"Doc\": {}{}}}", "[".repeat(depth), "]".repeat(depth));
    for body in [
        r#"{"Doc": "a\ud800b"}"#.to_owned(),
        nested(model::pyjson::MAX_DEPTH),
    ] {
        let failed = output(&[], body.as_bytes()).err();
        assert!(
            failed
                .as_ref()
                .is_some_and(|error| error.message.starts_with("native client limit: ")),
            "{failed:?}"
        );
    }
    assert!(output(&[], br#"{"Doc": [1,}"#).is_ok());
}

/// A document at the reader's limit parses and drops on a thread with the
/// default stack, the reader and the structure pass being no deeper.
#[test]
fn a_body_at_the_limit_parses_on_a_default_thread() -> Result<(), Box<dyn std::error::Error>> {
    let depth = model::pyjson::MAX_DEPTH - 1;
    let body = format!("{{\"Doc\": {}{}}}", "[".repeat(depth), "]".repeat(depth));
    let parsed = std::thread::spawn(move || output(&[], body.as_bytes()).map(|_| ()))
        .join()
        .map_err(|_| "the parse overflowed its stack")?;
    Ok(parsed?)
}

/// Headers read as botocore's `HeadersDict` gives them: a repeated one's
/// values joined; and bytes not UTF-8 fail with CPython's own text.
#[test]
fn headers_and_bytes_read_as_botocore_reads_them() -> Result<(), Box<dyn std::error::Error>> {
    let parsed = output(
        &[("x-amzn-RequestId", "a"), ("x-amzn-requestid", "b")],
        b"{}",
    )?;
    let id = parsed
        .get("ResponseMetadata")
        .and_then(|metadata| metadata.get("RequestId"))
        .and_then(Value::as_str);
    assert_eq!(id, Some("a, b"));
    let failed = output(&[], b"{\"Doc\": \"\xff\"}")
        .err()
        .map(|error| error.message);
    assert_eq!(
        failed.as_deref(),
        Some("'utf-8' codec can't decode byte 0xff in position 9: invalid start byte")
    );
    Ok(())
}
