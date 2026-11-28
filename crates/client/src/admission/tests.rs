use super::*;
use model::shapes::Model;

/// An operation with a trait the client does not implement is refused,
/// whichever trait it is; a plain one is not, nor one whose endpoint
/// discovery is optional, which botocore's default `auto` mode does not
/// discover for (`_requires_endpoint_discovery`).
#[test]
fn operations_with_unimplemented_traits_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    let model = Model::from_json(
        br#"{"metadata": {}, "shapes": {}, "operations": {
        "Plain": {"name": "Plain", "http": {"method": "POST", "requestUri": "/"}},
        "Checked": {"name": "Checked", "http": {"method": "POST", "requestUri": "/"},
            "httpChecksum": {"requestChecksumRequired": true}},
        "Required": {"name": "Required", "http": {"method": "POST", "requestUri": "/"},
            "httpChecksumRequired": true},
        "Discovered": {"name": "Discovered", "http": {"method": "POST", "requestUri": "/"},
            "endpointdiscovery": {"required": true}},
        "Optional": {"name": "Optional", "http": {"method": "POST", "requestUri": "/"},
            "endpointdiscovery": {}},
        "Endpoints": {"name": "Endpoints", "http": {"method": "POST", "requestUri": "/"},
            "endpointoperation": true},
        "Compressed": {"name": "Compressed", "http": {"method": "POST", "requestUri": "/"},
            "requestcompression": {"encodings": ["gzip"]}}
    }}"#,
    )?;
    for (name, refused) in [
        ("Plain", None),
        ("Checked", Some("httpChecksum")),
        ("Required", Some("httpChecksumRequired")),
        ("Discovered", Some("endpointdiscovery")),
        ("Optional", None),
        ("Endpoints", Some("endpointoperation")),
        ("Compressed", Some("requestcompression")),
    ] {
        let operation = model.schema().service().operation(name).ok_or(name)?;
        assert_eq!(
            unimplemented_trait(operation),
            refused.map(|name: &str| Unsupported::Trait(name.to_owned())),
            "{name}"
        );
    }
    Ok(())
}

const RULES: &str = r#"{"version": "1.0", "parameters": {
    "Region": {"type": "String", "builtIn": "AWS::Region", "required": false},
    "Thing": {"type": "String", "builtIn": "AWS::Other::Thing", "required": true}},
    "rules": [{"conditions": [], "error": "unused", "type": "error"}]}"#;

const SERVICE: &str = r#"{"metadata": {"protocol": "json", "jsonVersion": "1.0",
    "signatureVersion": "v4", "targetPrefix": "Z"},
    "operations": {
        "Plain": {"name": "Plain", "http": {"method": "POST", "requestUri": "/"}},
        "Streamed": {"name": "Streamed", "http": {"method": "POST", "requestUri": "/"},
            "output": {"shape": "Blobby"}},
        "Contexted": {"name": "Contexted", "http": {"method": "POST", "requestUri": "/"},
            "operationContextParams": {"Thing": {"path": "a.b"}}},
        "SignedA": {"name": "SignedA", "http": {"method": "POST", "requestUri": "/"},
            "auth": ["aws.auth#sigv4a"]},
        "Unsigned": {"name": "Unsigned", "http": {"method": "POST", "requestUri": "/"},
            "unsignedPayload": true}
    },
    "shapes": {
        "Blobby": {"type": "structure", "members": {"Body": {"shape": "Stream"}}},
        "Stream": {"type": "blob", "streaming": true}
    }
}"#;

/// Each refusal the model and the rule set decide alone, before any
/// configuration, input or credentials: the protocol, a streaming member,
/// context parameters, the auth type, an unsigned payload, and a required
/// built-in no one supplies.
#[test]
fn static_refusals_need_only_the_model_and_the_rule_set() -> Result<(), Box<dyn std::error::Error>>
{
    let model = Model::from_json(SERVICE.as_bytes())?;
    let data = EndpointData::from_documents(RULES.as_bytes().to_vec(), b"{}".to_vec(), None)?;
    let refusal = |name: &str| -> Result<Option<Unsupported>, String> {
        let operation = model.schema().service().operation(name).ok_or(name)?;
        Ok(static_refusal(&model, operation, &data, "zjson", &[], None))
    };
    assert_eq!(
        refusal("Streamed")?,
        Some(Unsupported::Trait("streaming".into()))
    );
    assert_eq!(
        refusal("Contexted")?,
        Some(Unsupported::OperationContextParams)
    );
    assert_eq!(refusal("SignedA")?, Some(Unsupported::Auth("v4a".into())));
    assert_eq!(refusal("Unsigned")?, Some(Unsupported::UnsignedPayload));
    assert_eq!(
        refusal("Plain")?,
        Some(Unsupported::Builtin("AWS::Other::Thing".into()))
    );
    let unknown = r#""zproto-wire""#;
    let unknown_model = Model::from_json(SERVICE.replace(r#""json""#, unknown).as_bytes())?;
    let plain = unknown_model
        .schema()
        .service()
        .operation("Plain")
        .ok_or("Plain")?;
    assert_eq!(
        static_refusal(&unknown_model, plain, &data, "zjson", &[], None),
        Some(Unsupported::Protocol("zproto-wire".into()))
    );
    Ok(())
}

/// A key the model loader does not interpret, which the operation reaches,
/// refuses it by that key: fail closed when a model gains a trait.
#[test]
fn an_uninterpreted_key_refuses_the_operations_that_reach_it()
-> Result<(), Box<dyn std::error::Error>> {
    let planted = SERVICE.replace(
        r#""Plain": {"name": "Plain","#,
        r#""Plain": {"name": "Plain", "newTrait": true,"#,
    );
    let model = Model::from_json(planted.as_bytes())?;
    let data = EndpointData::from_documents(RULES.as_bytes().to_vec(), b"{}".to_vec(), None)?;
    let plain = model.schema().service().operation("Plain").ok_or("Plain")?;
    assert_eq!(
        static_refusal(&model, plain, &data, "zjson", &[], None),
        Some(Unsupported::Trait("newTrait".into()))
    );
    Ok(())
}

/// A paginator entry the native paginator does not run refuses the
/// operation by its own reason: an expression beyond names and dotted
/// paths, or an entry botocore could not read. An entry it runs, or none,
/// refuses nothing.
#[test]
fn a_paginator_entry_the_engine_does_not_run_is_refused() -> Result<(), Box<dyn std::error::Error>>
{
    let model = Model::from_json(SERVICE.as_bytes())?;
    let rules = br#"{"version": "1.0", "parameters": {}, "rules": []}"#.to_vec();
    let data = EndpointData::from_documents(rules, b"{}".to_vec(), None)?;
    let operation = model.schema().service().operation("Plain").ok_or("Plain")?;
    let entry = |json: &str| {
        model::value::Value::from_json(json.as_bytes())
            .map(|entry| model::paginators::Config::from_value(&entry))
    };
    let refusal = |paginator| static_refusal(&model, operation, &data, "zjson", &[], paginator);
    assert_eq!(refusal(None), None);
    let tokens = r#""input_token": "NextToken", "output_token": "NextToken""#;
    let runs = entry(&format!(r#"{{{tokens}, "result_key": "Items"}}"#))?;
    assert_eq!(refusal(Some(&runs)), None);
    let filtered = entry(&format!(r#"{{{tokens}, "result_key": "Items[].Name"}}"#))?;
    assert_eq!(
        refusal(Some(&filtered)),
        Some(Unsupported::Paginator(
            model::paginators::Refused::Expression("Items[].Name".into())
        ))
    );
    let keyless = entry(&format!("{{{tokens}}}"))?;
    assert!(matches!(
        refusal(Some(&keyless)),
        Some(Unsupported::Paginator(
            model::paginators::Refused::Malformed(_)
        ))
    ));
    Ok(())
}

/// One operation for each way an end can stream, under `protocol`.
fn streaming_model(protocol: &str) -> Result<Model, Box<dyn std::error::Error>> {
    let text = r#"{"metadata": {"protocol": "PROTOCOL", "jsonVersion": "1.0",
        "signatureVersion": "v4", "targetPrefix": "Z"},
        "operations": {
            "Download": {"name": "Download", "http": {"method": "GET", "requestUri": "/d"},
                "output": {"shape": "StreamedOut"}},
            "Export": {"name": "Export", "http": {"method": "GET", "requestUri": "/e"},
                "output": {"shape": "BlobOut"}},
            "Upload": {"name": "Upload", "http": {"method": "PUT", "requestUri": "/u"},
                "input": {"shape": "StreamedIn"}},
            "Loose": {"name": "Loose", "http": {"method": "POST", "requestUri": "/l"},
                "output": {"shape": "LooseOut"}},
            "Events": {"name": "Events", "http": {"method": "POST", "requestUri": "/v"},
                "output": {"shape": "EventsOut"}}
        },
        "shapes": {
            "StreamedOut": {"type": "structure", "payload": "Body",
                "members": {"Body": {"shape": "Stream"}, "Kind": {"shape": "Text",
                    "location": "header", "locationName": "Content-Type"}}},
            "BlobOut": {"type": "structure", "payload": "Body",
                "members": {"Body": {"shape": "Bytes"}}},
            "StreamedIn": {"type": "structure", "payload": "Body",
                "members": {"Body": {"shape": "Stream"}}},
            "LooseOut": {"type": "structure", "members": {"Body": {"shape": "Stream"}}},
            "EventsOut": {"type": "structure", "payload": "Events",
                "members": {"Events": {"shape": "Events"}}},
            "Events": {"type": "structure", "eventstream": true,
                "members": {"Tick": {"shape": "Tick"}}},
            "Tick": {"type": "structure", "event": true, "members": {}},
            "Stream": {"type": "blob", "streaming": true},
            "Bytes": {"type": "blob"},
            "Text": {"type": "string"}
        }
    }"#;
    Ok(Model::from_json(
        text.replace("PROTOCOL", protocol).as_bytes(),
    )?)
}

/// A blob payload, marked `streaming` or not, is carried at either end
/// under the REST protocols, which bind a payload; a `streaming` member
/// that is no payload, a blob payload under another protocol, and an event
/// stream are refused.
#[test]
fn a_blob_payload_streams_only_where_a_protocol_binds_it() -> Result<(), Box<dyn std::error::Error>>
{
    let rules = br#"{"version": "1.0", "parameters": {}, "rules": []}"#.to_vec();
    let data = EndpointData::from_documents(rules, b"{}".to_vec(), None)?;
    let streaming = Some(Unsupported::Trait("streaming".into()));
    let events = Some(Unsupported::Trait("eventstream".into()));
    for (protocol, carried) in [("rest-json", true), ("rest-xml", true), ("json", false)] {
        let model = streaming_model(protocol)?;
        for (name, refused) in [
            ("Download", if carried { None } else { streaming.clone() }),
            ("Export", if carried { None } else { streaming.clone() }),
            ("Upload", if carried { None } else { streaming.clone() }),
            ("Loose", streaming.clone()),
            ("Events", events.clone()),
        ] {
            let operation = model.schema().service().operation(name).ok_or(name)?;
            assert_eq!(
                static_refusal(&model, operation, &data, "zjson", &[], None),
                refused,
                "{protocol} {name}"
            );
        }
    }
    Ok(())
}
