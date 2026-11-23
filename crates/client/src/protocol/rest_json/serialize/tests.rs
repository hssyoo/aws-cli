//! The quirks botocore's protocol suite does not pin, each expectation what
//! botocore's `RestJSONSerializer` and `prepare_request_dict` give for the
//! same model and input.
use super::*;
use timestamp::Timestamp;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const MODEL: &str = r#"{
  "metadata": {"apiVersion": "2020-01-01", "protocol": "rest-json", "endpointPrefix": "svc"},
  "operations": {
    "Labels": {"name": "Labels", "input": {"shape": "LabelsIn"},
      "http": {"method": "GET", "requestUri": "/l/{Name}/{Key+}/{When}/{Flag}/{Data}?tagging"}},
    "Bare": {"name": "Bare", "http": {"method": "DELETE", "requestUri": "/bare/{Id}?x=1"}},
    "Query": {"name": "Query", "http": {"method": "GET", "requestUri": "/q"},
      "input": {"shape": "QueryIn"}},
    "Heads": {"name": "Heads", "http": {"method": "POST", "requestUri": "/h"},
      "input": {"shape": "HeadsIn"}},
    "Located": {"name": "Located", "http": {"method": "POST", "requestUri": "/located"},
      "input": {"shape": "LocatedIn"}},
    "Body": {"name": "Body", "http": {"method": "PUT", "requestUri": "/b"},
      "input": {"shape": "BodyIn"}},
    "Blob": {"name": "Blob", "http": {"method": "PUT", "requestUri": "/blob"},
      "input": {"shape": "BlobIn"}},
    "Struct": {"name": "Struct", "http": {"method": "PUT", "requestUri": "/s"},
      "input": {"shape": "StructIn"}},
    "Doc": {"name": "Doc", "http": {"method": "PUT", "requestUri": "/d"},
      "input": {"shape": "DocIn"}},
    "NoUri": {"name": "NoUri", "http": {"method": "POST"}, "input": {"shape": "LocatedIn"}}
  },
  "shapes": {
    "LabelsIn": {"type": "structure", "members": {
      "Name": {"shape": "S", "location": "uri", "locationName": "Name"},
      "Key": {"shape": "S", "location": "uri", "locationName": "Key"},
      "When": {"shape": "Epoch", "location": "uri", "locationName": "When"},
      "Flag": {"shape": "B", "location": "uri", "locationName": "Flag"},
      "Data": {"shape": "Bl", "location": "uri", "locationName": "Data"},
      "Tags": {"shape": "SL", "location": "querystring", "locationName": "tag"}}},
    "QueryIn": {"type": "structure", "members": {
      "Text": {"shape": "S", "location": "querystring", "locationName": "text"},
      "Extra": {"shape": "SM", "location": "querystring"},
      "Count": {"shape": "I", "location": "querystring", "locationName": "count"},
      "Flags": {"shape": "BL", "location": "querystring", "locationName": "flag"},
      "Since": {"shape": "T", "location": "querystring", "locationName": "since"},
      "Ratio": {"shape": "D", "location": "querystring", "locationName": "ratio"},
      "Data": {"shape": "Bl", "location": "querystring", "locationName": "data"}}},
    "HeadsIn": {"type": "structure", "members": {
      "When": {"shape": "T", "location": "header", "locationName": "X-When"},
      "Iso": {"shape": "Iso", "location": "header", "locationName": "X-Iso"},
      "Epoch": {"shape": "Epoch", "location": "header", "locationName": "X-Epoch"},
      "Json": {"shape": "S", "location": "header", "locationName": "X-Json", "jsonvalue": true},
      "Names": {"shape": "SL", "location": "header", "locationName": "X-Names"},
      "Times": {"shape": "TL", "location": "header", "locationName": "X-Times"},
      "Empty": {"shape": "SL", "location": "header", "locationName": "X-Empty"},
      "Flag": {"shape": "B", "location": "header", "locationName": "X-Flag"},
      "Ratio": {"shape": "D", "location": "header", "locationName": "X-Ratio"},
      "Count": {"shape": "I", "location": "header", "locationName": "X-Count"},
      "Data": {"shape": "Bl", "location": "header", "locationName": "X-Data"},
      "Meta": {"shape": "SM", "location": "headers", "locationName": "X-Meta-"}}},
    "LocatedIn": {"type": "structure", "members": {
      "Id": {"shape": "S", "location": "header", "locationName": "X-Id"}}},
    "BodyIn": {"type": "structure", "members": {
      "Id": {"shape": "S", "location": "header", "locationName": "X-Id"},
      "Kind": {"shape": "S", "location": "header", "locationName": "content-type"},
      "Note": {"shape": "S", "locationName": "note"}}},
    "BlobIn": {"type": "structure", "payload": "Data", "members": {
      "Kind": {"shape": "S", "location": "header", "locationName": "Content-Type"},
      "Data": {"shape": "Bl"}}},
    "StructIn": {"type": "structure", "payload": "Inner", "members": {
      "Inner": {"shape": "Inner"}, "Ignored": {"shape": "S"}}},
    "DocIn": {"type": "structure", "payload": "Doc", "members": {"Doc": {"shape": "Document"}}},
    "Inner": {"type": "structure", "members": {"A": {"shape": "S"}}},
    "Document": {"type": "structure", "members": {}, "document": true},
    "S": {"type": "string"}, "I": {"type": "integer"}, "B": {"type": "boolean"},
    "D": {"type": "double"}, "Bl": {"type": "blob"}, "T": {"type": "timestamp"},
    "Iso": {"type": "timestamp", "timestampFormat": "iso8601"},
    "Epoch": {"type": "timestamp", "timestampFormat": "unixTimestamp"},
    "SL": {"type": "list", "member": {"shape": "S"}},
    "BL": {"type": "list", "member": {"shape": "B"}},
    "TL": {"type": "list", "member": {"shape": "T"}},
    "SM": {"type": "map", "key": {"shape": "S"}, "value": {"shape": "S"}}
  }
}"#;

/// The request botocore would build for `operation` with `input`.
fn serialize(operation: &str, input: Value) -> Result<Serialized, Box<dyn std::error::Error>> {
    let model = Model::from_json(MODEL.as_bytes())?;
    let operation = model.operation(operation).ok_or("operation")?;
    Ok(request(&model, operation, &input)?)
}

/// The error serializing `input` gives.
fn refused(operation: &str, input: Value) -> Result<String, Box<dyn std::error::Error>> {
    let model = Model::from_json(MODEL.as_bytes())?;
    let operation = model.operation(operation).ok_or("operation")?;
    match request(&model, operation, &input) {
        Ok(_) => Err("serialized".into()),
        Err(error) => Ok(error.message),
    }
}

fn params(pairs: Vec<(&str, Value)>) -> Value {
    Value::Map(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn text(value: &str) -> Value {
    Value::String(value.into())
}

fn texts(values: &[&str]) -> Value {
    Value::List(values.iter().copied().map(text).collect())
}

/// 2015-08-30T12:36:00.5Z.
fn half_past() -> Result<Value, Box<dyn std::error::Error>> {
    let at = Timestamp::from_unix_micros(1_440_938_160_500_000).ok_or("timestamp")?;
    Ok(Value::Timestamp(at))
}

fn headers(request: &Serialized) -> Vec<(&str, &str)> {
    request.headers.iter().collect()
}

/// `{Label}` encodes `/`, a greedy `{Label+}` keeps it; `~` stays; a blob
/// label is its raw bytes; a unix timestamp label is its `str()`; the
/// `requestUri`'s own query goes before the members'.
#[test]
fn labels_are_percent_encoded_as_botocore_encodes_them() -> TestResult {
    let request = serialize(
        "Labels",
        params(vec![
            ("Name", text("a b/c~ü")),
            ("Key", text("x/y z")),
            ("When", half_past()?),
            ("Flag", Value::Bool(false)),
            ("Data", Value::Blob(vec![0, b'/', 0xff])),
            ("Tags", texts(&["p", "q"])),
        ]),
    )?;
    assert_eq!(request.method.as_str(), "GET");
    assert_eq!(
        request.path,
        "/l/a%20b%2Fc~%C3%BC/x/y%20z/1440938160.5/false/%00%2F%FF"
    );
    assert_eq!(request.query, "tagging&tag=p&tag=q");
    assert!(request.headers.is_empty());
    assert!(request.body.is_empty());
    Ok(())
}

/// `prepare_request_dict` adds `&` after the `requestUri`'s query whenever
/// the query string has a name, even one whose empty list adds no pair.
#[test]
fn an_empty_list_still_adds_the_ampersand() -> TestResult {
    let base = vec![
        ("Name", text("n")),
        ("Key", text("k")),
        ("When", Value::Integer(1_440_938_160)),
        ("Flag", Value::Bool(true)),
        ("Data", Value::Blob(b"d".to_vec())),
    ];
    let mut with_empty = base.clone();
    with_empty.push(("Tags", Value::List(Vec::new())));
    let request = serialize("Labels", params(with_empty))?;
    assert_eq!(request.path, "/l/n/k/1440938160/true/d");
    assert_eq!(request.query, "tagging&");
    assert_eq!(serialize("Labels", params(base))?.query, "tagging");
    Ok(())
}

/// With no input shape the `requestUri` is used as it is, labels unfilled.
#[test]
fn an_operation_without_input_keeps_its_template() -> TestResult {
    let request = serialize("Bare", Value::Map(Map::new()))?;
    assert_eq!(request.method.as_str(), "DELETE");
    assert_eq!(request.path, "/bare/{Id}");
    assert_eq!(request.query, "x=1");
    assert!(request.headers.is_empty() && request.body.is_empty());
    Ok(())
}

/// Pairs in the caller's order; a map's entries replace a named member's
/// value in its place (`dict.update`); booleans lower case, timestamps
/// ISO 8601 with microseconds, NaN spelled out, blobs as raw bytes, and
/// every byte but `-._~` and alphanumerics percent-encoded.
#[test]
fn the_query_string_keeps_botocore_s_order_and_encoding() -> TestResult {
    let extra = params(vec![("text", text("fromMap")), ("other", text("a&b=c"))]);
    let request = serialize(
        "Query",
        params(vec![
            ("Text", text("named")),
            ("Extra", extra),
            ("Count", Value::Integer(3)),
            (
                "Flags",
                Value::List(vec![Value::Bool(true), Value::Bool(false)]),
            ),
            ("Since", half_past()?),
            ("Ratio", Value::Float(f64::NAN)),
            ("Data", Value::Blob(vec![1, b' ', 0xfe])),
        ]),
    )?;
    assert_eq!(
        request.query,
        "text=fromMap&other=a%26b%3Dc&count=3&flag=true&flag=false\
         &since=2015-08-30T12%3A36%3A00.500000Z&ratio=NaN&data=%01%20%FE"
    );
    let request = serialize(
        "Query",
        params(vec![
            ("Count", Value::Integer(0)),
            ("Text", Value::Null),
            ("Ratio", Value::Float(1.5)),
        ]),
    )?;
    assert_eq!(request.query, "count=0&ratio=1.5");
    Ok(())
}

/// Header timestamps to the second (`rfc822` by default); `jsonvalue` as
/// base64 of compact, ASCII-only JSON; list items joined by `,`, strings
/// quoted when they hold `,` or `"`, `None` items and empty lists left out;
/// a blob as its `str()`; prefix headers after the prefix.
#[test]
fn header_values_take_botocore_s_formats() -> TestResult {
    let json = params(vec![
        ("é", Value::List(vec![Value::Integer(1), text("two")])),
        ("b", Value::Null),
    ]);
    let mut names = texts(&["plain", "a,b", "say \"hi\""]);
    if let Value::List(items) = &mut names {
        items.push(Value::Null);
    }
    let request = serialize(
        "Heads",
        params(vec![
            ("When", half_past()?),
            ("Iso", half_past()?),
            ("Epoch", half_past()?),
            ("Json", json),
            ("Names", names),
            ("Times", Value::List(vec![half_past()?, Value::Integer(0)])),
            ("Empty", Value::List(Vec::new())),
            ("Flag", Value::Bool(false)),
            ("Ratio", Value::Float(f64::INFINITY)),
            ("Count", Value::Integer(7)),
            ("Data", Value::Blob(b"raw".to_vec())),
            ("Meta", params(vec![("a", text("1")), ("B", text("2"))])),
        ]),
    )?;
    let times = "Sun, 30 Aug 2015 12:36:00 GMT,Thu, 01 Jan 1970 00:00:00 GMT";
    assert_eq!(
        headers(&request),
        [
            ("X-When", "Sun, 30 Aug 2015 12:36:00 GMT"),
            ("X-Iso", "2015-08-30T12:36:00Z"),
            ("X-Epoch", "1440938160"),
            ("X-Json", "eyJcdTAwZTkiOlsxLCJ0d28iXSwiYiI6bnVsbH0="),
            ("X-Names", r#"plain,"a,b","say \"hi\"""#),
            ("X-Times", times),
            ("X-Flag", "false"),
            ("X-Ratio", "Infinity"),
            ("X-Count", "7"),
            ("X-Data", "b'raw'"),
            ("X-Meta-a", "1"),
            ("X-Meta-B", "2"),
        ]
    );
    assert!(request.body.is_empty());
    Ok(())
}

/// Only location members: no body and no `Content-Type`. A body member
/// unset: `{}` (`_requires_empty_body`), as JSON.
#[test]
fn an_empty_body_is_nothing_or_an_empty_object() -> TestResult {
    for input in [Value::Map(Map::new()), params(vec![("Id", text("i"))])] {
        let request = serialize("Located", input)?;
        assert!(request.body.is_empty());
        assert!(!request.headers.contains("Content-Type"));
    }
    let request = serialize("Body", params(vec![("Id", text("i"))]))?;
    assert_eq!(request.body, b"{}");
    assert_eq!(
        headers(&request),
        [("X-Id", "i"), ("Content-Type", "application/json")]
    );
    Ok(())
}

/// A JSON body is `application/json` unless a member set `Content-Type`,
/// in any case.
#[test]
fn a_member_s_content_type_wins() -> TestResult {
    let request = serialize("Body", params(vec![("Note", text("n"))]))?;
    assert_eq!(request.body, br#"{"note": "n"}"#);
    assert_eq!(headers(&request), [("Content-Type", "application/json")]);
    let request = serialize(
        "Body",
        params(vec![("Note", text("n")), ("Kind", text("text/plain"))]),
    )?;
    assert_eq!(headers(&request), [("content-type", "text/plain")]);
    Ok(())
}

/// A blob payload goes as its bytes, text as UTF-8, unset as nothing, and
/// never with a `Content-Type` of botocore's.
#[test]
fn a_blob_payload_goes_raw() -> TestResult {
    let request = serialize("Blob", Value::Map(Map::new()))?;
    assert!(request.body.is_empty() && request.headers.is_empty());
    let request = serialize(
        "Blob",
        params(vec![("Data", Value::Blob(b"\0bytes".to_vec()))]),
    )?;
    assert_eq!(request.body, b"\0bytes");
    assert!(request.headers.is_empty());
    let request = serialize(
        "Blob",
        params(vec![("Data", text("text")), ("Kind", text("image/png"))]),
    )?;
    assert_eq!(request.body, b"text");
    assert_eq!(headers(&request), [("Content-Type", "image/png")]);
    Ok(())
}

/// A structure payload is its members alone, `{}` when unset; a document
/// payload is under the `None` key `json.dumps` writes as `"null"`.
#[test]
fn a_structured_payload_goes_as_json() -> TestResult {
    let request = serialize("Struct", params(vec![("Ignored", text("x"))]))?;
    assert_eq!(request.body, b"{}");
    assert_eq!(headers(&request), [("Content-Type", "application/json")]);
    let inner = params(vec![("A", text("a"))]);
    let request = serialize(
        "Struct",
        params(vec![("Inner", inner), ("Ignored", text("x"))]),
    )?;
    assert_eq!(request.body, br#"{"A": "a"}"#);
    let document = Value::List(vec![Value::Integer(1), params(vec![("k", text("v"))])]);
    let request = serialize("Doc", params(vec![("Doc", document)]))?;
    assert_eq!(request.body, br#"{"null": [1, {"k": "v"}]}"#);
    assert_eq!(headers(&request), [("Content-Type", "application/json")]);
    Ok(())
}

/// The errors Python raises: `KeyError` for a member, label or
/// `requestUri` that is not there, `AttributeError` for an input that is
/// not a dict.
#[test]
fn missing_names_are_key_errors() -> TestResult {
    assert_eq!(
        refused("Located", params(vec![("Nope", text("x"))]))?,
        "'Nope'"
    );
    assert_eq!(
        refused("Labels", params(vec![("Name", text("n"))]))?,
        "'Key'"
    );
    assert_eq!(refused("NoUri", Value::Map(Map::new()))?, "'requestUri'");
    assert_eq!(
        refused("Located", Value::List(Vec::new()))?,
        "'list' object has no attribute 'items'"
    );
    Ok(())
}

/// Values a location cannot hold, refused with the text Python's error has
/// (a bad timestamp's as the shared timestamp conversion words it). A blob
/// payload that is not bytes or text, which botocore leaves for the HTTP
/// layer to fail on, is refused here; validation lets neither through.
#[test]
fn values_a_location_cannot_hold_are_refused() -> TestResult {
    assert_eq!(
        refused("Heads", params(vec![("When", text("soon"))]))?,
        "Invalid timestamp \"soon\""
    );
    assert_eq!(
        refused("Heads", params(vec![("Json", Value::Blob(b"x".to_vec()))]))?,
        "Object of type bytes is not JSON serializable"
    );
    assert_eq!(
        refused(
            "Heads",
            params(vec![("Count", Value::Integer(1)), ("Meta", text("m"))])
        )?,
        "'str' object has no attribute 'items'"
    );
    assert_eq!(
        refused("Heads", params(vec![("Names", Value::Integer(1))]))?,
        "'int' object is not iterable"
    );
    assert_eq!(
        refused("Blob", params(vec![("Data", Value::Integer(1))]))?,
        "a bytes-like object is required, not 'int'"
    );
    assert_eq!(
        refused("Struct", params(vec![("Inner", text("x"))]))?,
        "'str' object has no attribute 'items'"
    );
    Ok(())
}
