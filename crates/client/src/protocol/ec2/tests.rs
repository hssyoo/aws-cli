use super::*;
use crate::admission::Unsupported;
use crate::protocol::{IMPLEMENTED, SWITCHED_OFF, compat, select};
use model::schema::TypeRegistry;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const MODEL: &str = r#"{
  "metadata": {"apiVersion": "2020-01-01", "protocol": "ec2", "protocols": ["ec2"]},
  "operations": {
    "Go": {"name": "Go", "http": {"method": "POST", "requestUri": "/"},
           "input": {"shape": "In"}, "output": {"shape": "Out"},
           "errors": [{"shape": "Bad"}]}
  },
  "shapes": {
    "In": {"type": "structure", "members": {
      "Name": {"shape": "S", "locationName": "name"},
      "Query": {"shape": "S", "locationName": "ignored", "queryName": "Q"},
      "Plain": {"shape": "S"},
      "lower": {"shape": "S"},
      "Sharp": {"shape": "S", "locationName": "ßeta"},
      "Ids": {"shape": "List", "locationName": "id"},
      "Flat": {"shape": "FlatList"},
      "Empty": {"shape": "List"},
      "Tags": {"shape": "Map"},
      "Inner": {"shape": "Inner", "locationName": "inner"}
    }},
    "Inner": {"type": "structure", "members": {"Zone": {"shape": "S", "locationName": "zone"}}},
    "List": {"type": "list", "member": {"shape": "S", "locationName": "item"}},
    "FlatList": {"type": "list", "member": {"shape": "S", "locationName": "entry"},
      "flattened": true},
    "Map": {"type": "map", "key": {"shape": "S", "locationName": "k"},
      "value": {"shape": "S", "locationName": "v"}},
    "Out": {"type": "structure", "members": {"Value": {"shape": "S", "locationName": "value"}}},
    "S": {"type": "string"},
    "Bad": {"type": "structure", "members": {"Detail": {"shape": "S"}, "Code": {"shape": "S"}},
      "error": {"code": "Bad"}, "exception": true}
  }
}"#;

fn model() -> Result<Model> {
    Ok(Model::from_json(MODEL.as_bytes())?)
}

fn go(model: &Model) -> Result<OperationSchema<'_>> {
    Ok(model.schema().service().operation("Go").ok_or("Go")?)
}

fn input(json: &str) -> Value {
    Value::from_json(json.as_bytes()).unwrap_or(Value::Null)
}

fn response(status: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        headers: Headers::new(),
        body: body.as_bytes().to_vec(),
    }
}

fn body(model: &Model, json: &str) -> Result<String> {
    let request = EC2_QUERY.serialize_request(go(model)?, &input(json), model)?;
    Ok(String::from_utf8(request.body)?)
}

/// `queryName` wins, a `locationName` is capitalized (Python's full
/// uppercase), a member's own name is kept as it is, nested names join.
#[test]
fn names_are_query_names_or_capitalized_location_names() -> Result<()> {
    let model = model()?;
    let json = r#"{"Name": "a", "Query": "b", "Plain": "c", "lower": "d", "Sharp": "e",
                   "Inner": {"Zone": "z"}}"#;
    assert_eq!(
        body(&model, json)?,
        "Action=Go&Version=2020-01-01&Name=a&Q=b&Plain=c&lower=d&SSeta=e&Inner.Zone=z"
    );
    Ok(())
}

/// A list's items are `Name.N` with neither `member`, the item's
/// `locationName` nor the flattened rule, and an empty list writes nothing.
#[test]
fn lists_are_numbered_after_their_own_name() -> Result<()> {
    let model = model()?;
    let json = r#"{"Ids": ["x", "y"], "Flat": ["f"], "Empty": []}"#;
    assert_eq!(
        body(&model, json)?,
        "Action=Go&Version=2020-01-01&Id.1=x&Id.2=y&Flat.1=f"
    );
    Ok(())
}

/// A map keeps the query layout, its key and value names capitalized.
#[test]
fn maps_capitalize_their_entry_names() -> Result<()> {
    let model = model()?;
    assert_eq!(
        body(&model, r#"{"Tags": {"a": "1"}}"#)?,
        "Action=Go&Version=2020-01-01&Tags.entry.1.K=a&Tags.entry.1.V=1"
    );
    let request = EC2_QUERY.serialize_request(go(&model)?, &input("{}"), &model)?;
    assert_eq!(
        request.headers.get("Content-Type"),
        Some("application/x-www-form-urlencoded; charset=utf-8")
    );
    assert_eq!((request.method.as_str(), &*request.path), ("POST", "/"));
    Ok(())
}

/// The output is read from the root, with `requestId` as the metadata and
/// a `ResponseMetadata` element ignored.
#[test]
fn the_request_id_is_the_root_s_request_id() -> Result<()> {
    let model = model()?;
    let body = "<GoResponse><requestId>r-1</requestId><value>v</value>\
                <ResponseMetadata><RequestId>no</RequestId></ResponseMetadata></GoResponse>";
    let parsed = EC2_QUERY.parse(go(&model)?, &response(200, body), &ParseOptions::BOTOCORE)?;
    let Value::Map(parsed) = parsed else {
        return Err("not a map".into());
    };
    assert_eq!(parsed.get("Value"), Some(&Value::String("v".into())));
    let Some(Value::Map(metadata)) = parsed.get("ResponseMetadata") else {
        return Err("no metadata".into());
    };
    assert_eq!(
        metadata.get("RequestId"),
        Some(&Value::String("r-1".into()))
    );
    let quiet = "<GoResponse><ResponseMetadata><RequestId>no</RequestId></ResponseMetadata>\
                 </GoResponse>";
    let parsed = EC2_QUERY.parse(go(&model)?, &response(200, quiet), &ParseOptions::BOTOCORE)?;
    let Value::Map(parsed) = parsed else {
        return Err("not a map".into());
    };
    let Some(Value::Map(metadata)) = parsed.get("ResponseMetadata") else {
        return Err("no metadata".into());
    };
    assert!(metadata.get("RequestId").is_none());
    Ok(())
}

const ERROR: &str = "<Response><Errors><Error><Code>Bad</Code><Message>m</Message>\
                     <Detail>d</Detail></Error></Errors><RequestID>r-2</RequestID></Response>";

/// The envelope's `Error` is the error, `RequestID` its request id, and a
/// modelled error's members come from the same `Error`.
#[test]
fn errors_are_read_from_the_errors_envelope() -> Result<()> {
    let model = model()?;
    let registry: TypeRegistry<'_> = model.schema().registry();
    let error = EC2_QUERY.parse_error(
        go(&model)?,
        &response(400, ERROR),
        &ParseOptions::BOTOCORE,
        registry,
    )?;
    assert_eq!(error.error.get("Code"), Some(&Value::String("Bad".into())));
    assert_eq!(error.error.get("Message"), Some(&Value::String("m".into())));
    assert_eq!(error.request_id.as_deref(), Some("r-2"));
    assert_eq!(
        error.members.get("Detail"),
        Some(&Value::String("d".into()))
    );
    assert_eq!(
        error.members.get("Code"),
        Some(&Value::String("Bad".into()))
    );
    // The awsQuery parser reads neither the id nor the members.
    let query = AWS_QUERY.parse_error(
        go(&model)?,
        &response(400, ERROR),
        &ParseOptions::BOTOCORE,
        registry,
    )?;
    assert_eq!(query.request_id, None);
    assert_eq!(query.members.get("Detail"), None);
    Ok(())
}

/// `_get_error_root` skips an `Errors` without an `Error` for a later one,
/// ignores the root's name, and falls back to the root.
#[test]
fn the_error_root_is_the_first_error_of_an_errors() -> Result<()> {
    let model = model()?;
    let bad = model.schema().shape_named("Bad").ok_or("Bad")?;
    let read = |body: &str| {
        EC2_QUERY.deserialize_modeled_error(bad, &response(400, body), &ParseOptions::BOTOCORE)
    };
    let later = "<Any><Errors/><Errors><Other/><Error><Detail>2</Detail></Error></Errors></Any>";
    assert_eq!(read(later)?.get("Detail"), Some(&Value::String("2".into())));
    let bare = "<Response><Detail>root</Detail></Response>";
    assert_eq!(
        read(bare)?.get("Detail"),
        Some(&Value::String("root".into()))
    );
    let query = "<ErrorResponse><Error><Detail>q</Detail></Error></ErrorResponse>";
    assert_eq!(read(query)?.get("Detail"), None);
    Ok(())
}

/// A `RequestID` replaces a `RequestId`'s metadata, which keeps its place; a 5xx with an
/// empty body is the generic error.
#[test]
fn request_id_spellings_and_generic_errors() -> Result<()> {
    let both = "<Response><RequestId>a</RequestId><Errors><Error><Code>C</Code></Error></Errors>\
                <RequestID>b</RequestID></Response>";
    let parsed = EC2_QUERY.parse_error_metadata(&response(400, both))?;
    let keys: Vec<_> = parsed.iter().filter_map(|(key, _)| key.as_str()).collect();
    assert_eq!(keys, ["Error", "ResponseMetadata"]);
    let Some(Value::Map(metadata)) = parsed.get("ResponseMetadata") else {
        return Err("no metadata".into());
    };
    assert_eq!(metadata.get("RequestId"), Some(&Value::String("b".into())));
    let model = model()?;
    let generic = compat::parse(
        &EC2_QUERY,
        go(&model)?,
        None,
        &response(503, ""),
        &ParseOptions::BOTOCORE,
    )?;
    let Value::Map(generic) = generic else {
        return Err("not a map".into());
    };
    let Some(Value::Map(error)) = generic.get("Error") else {
        return Err("no error".into());
    };
    assert_eq!(error.get("Code"), Some(&Value::String("503".into())));
    Ok(())
}

/// Switched off: admission and clients still refuse `ec2`; only the
/// switched-off registry holds it, with iso8601 timestamps.
#[test]
fn ec2_is_built_but_not_admitted() -> Result<()> {
    let model = model()?;
    assert!(matches!(select(&model), Err(Unsupported::Protocol(name)) if name == "ec2"));
    assert!(IMPLEMENTED.named("ec2").is_none());
    assert!(IMPLEMENTED.get(traits::EC2_QUERY).is_none());
    let built = SWITCHED_OFF
        .resolve(model.metadata())
        .map_err(|unsupported| format!("{unsupported:?}"))?;
    assert_eq!(built.protocol_id(), traits::EC2_QUERY);
    assert!(!compat::timestamps_epoch(built));
    assert!(EC2_QUERY.payload_codec().is_none());
    Ok(())
}
