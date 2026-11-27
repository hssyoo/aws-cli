use super::*;
use crate::protocol::{AWS_JSON_1_0, AWS_QUERY};
use model::shapes::Model;

fn response(status: u16, body: &[u8]) -> HttpResponse {
    HttpResponse {
        status,
        headers: Headers::from_pairs([
            ("X-Amzn-RequestId", "r1"),
            ("Set-Cookie", "a"),
            ("set-cookie", "b"),
        ]),
        body: body.to_vec(),
    }
}

/// `_is_generic_error_response` and `_do_generic_error_parse`: a 5xx with
/// no body or an HTML one is the status and its reason phrase.
#[test]
fn a_bodiless_or_html_5xx_is_the_generic_error() -> Result<(), &'static str> {
    assert!(is_generic_error(&response(503, b"  ")));
    assert!(is_generic_error(&response(
        500,
        b"\n<html><body>x</body></html>"
    )));
    assert!(!is_generic_error(&response(500, b"<Error/>")));
    assert!(!is_generic_error(&response(404, b"")));
    let parsed = generic_error(503);
    let Some(Value::Map(error)) = parsed.get("Error") else {
        return Err("no Error");
    };
    assert_eq!(error.get("Code"), Some(&Value::String("503".into())));
    assert_eq!(
        error.get("Message"),
        Some(&Value::String("Service Unavailable".into()))
    );
    assert_eq!(reason_phrase(599), "");
    Ok(())
}

/// `lowercase_dict` of headers as urllib3 joins a repeated one.
#[test]
fn response_headers_lowercase_and_join() {
    let headers = lowercase_headers(&response(200, b"").headers);
    assert_eq!(
        headers.get("x-amzn-requestid"),
        Some(&Value::String("r1".into()))
    );
    assert_eq!(
        headers.get("set-cookie"),
        Some(&Value::String("a, b".into()))
    );
}

/// Arbitrary bodies at every status reach an output or an error through
/// both protocols, never a panic: truncated and malformed XML and JSON,
/// invalid UTF-8, deep nesting, huge numbers.
#[test]
fn adversarial_bodies_never_panic() -> Result<(), Box<dyn std::error::Error>> {
    let model = Model::from_json(
        br#"{"metadata":{"apiVersion":"1","jsonVersion":"1.0","targetPrefix":"T"},
        "operations":{"Go":{"name":"Go","output":{"shape":"Out","resultWrapper":"GoResult"}}},
        "shapes":{"Out":{"type":"structure","members":{"N":{"shape":"I"},"L":{"shape":"L"},
        "M":{"shape":"M"},"T":{"shape":"Ts"},"B":{"shape":"Bl"}}},"I":{"type":"integer"},
        "L":{"type":"list","member":{"shape":"Out"}},"M":{"type":"map","key":{"shape":"S"},
        "value":{"shape":"Out"}},"S":{"type":"string"},"Ts":{"type":"timestamp"},
        "Bl":{"type":"blob"},"E":{"type":"structure","members":{},"exception":true}}}"#,
    )?;
    let operation = model
        .schema()
        .service()
        .operations()
        .next()
        .ok_or("operation")?;
    let deep_xml = [
        b"<GoResponse><GoResult>".to_vec(),
        b"<L><member>".repeat(400),
    ]
    .concat();
    let deep_json = [b"{\"L\":[".repeat(5_000), b"]}".repeat(5_000)].concat();
    let bodies: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"\xff\xfe".to_vec(),
        b"<GoResponse><GoResult><N>99999999999999999999999</N><T>never</T><B>!</B>".to_vec(),
        b"<ErrorResponse><Errors>x</Errors></ErrorResponse>".to_vec(),
        b"<GoResponse><GoResult><M><entry><key>k</key></entry></M></GoResult></GoResponse>"
            .to_vec(),
        br#"{"N":"x","L":{"a":1},"M":[1],"T":[],"B":5,"__type":7}"#.to_vec(),
        br#"{"N":1e999999,"T":1e300}"#.to_vec(),
        deep_xml,
        deep_json,
    ];
    let protocols: [&dyn ClientProtocol; 2] = [&AWS_QUERY, &AWS_JSON_1_0];
    for body in &bodies {
        for status in [200, 300, 400, 500] {
            let response = HttpResponse {
                status,
                headers: Headers::from_pairs([("x-amzn-query-error", ";;")]),
                body: body.clone(),
            };
            for protocol in protocols {
                let options = &ParseOptions::BOTOCORE;
                let _ = protocol.parse(operation, &response, options);
                let registry = model.schema().registry();
                let _ = protocol.parse_error(operation, &response, options, registry);
            }
        }
    }
    Ok(())
}
