use super::*;
use crate::source::{DirSource, Kind, Source};
use crate::testing::{self, Result};
use crate::value::Value;

fn fixture() -> Result<Model> {
    Ok(Model::from_json(include_bytes!("fixture.json"))?)
}

fn named(model: &Model, name: &str) -> Result<ShapeId> {
    Ok(model.shape_named(name).ok_or_else(|| format!("no shape {name}"))?)
}

fn member(model: &Model, shape: ShapeId, name: &str) -> Result<ShapeId> {
    let found = model.members(shape).iter().find(|member| &*member.name == name);
    Ok(found.ok_or_else(|| format!("no member {name}"))?.shape)
}

fn member_names(model: &Model, shape: ShapeId) -> Vec<&str> {
    model.members(shape).iter().map(|member| &*member.name).collect()
}

#[test]
fn metadata_and_client_context_parameters_are_read() -> Result<()> {
    let model = fixture()?;
    let metadata = model.metadata();
    assert_eq!(metadata.api_version.as_deref(), Some("2020-01-01"));
    assert_eq!(metadata.endpoint_prefix.as_deref(), Some("x"));
    assert_eq!(metadata.protocol.as_deref(), Some("json"));
    assert_eq!(metadata.protocols.as_deref(), Some(&["json".into(), "query".into()][..]));
    assert_eq!(metadata.json_version.as_deref(), Some("1.1"));
    assert_eq!(metadata.target_prefix.as_deref(), Some("X"));
    assert_eq!(metadata.service_id.as_deref(), Some("X Svc"));
    assert_eq!(metadata.service_full_name.as_deref(), Some("X Service"));
    assert_eq!(metadata.signature_version.as_deref(), Some("v4"));
    assert_eq!(metadata.signing_name.as_deref(), Some("xs"));
    assert_eq!(metadata.uid.as_deref(), Some("x-2020-01-01"));
    assert!(metadata.aws_query_compatible);
    assert_eq!(metadata.auth.as_deref(), Some(&["aws.auth#sigv4".into()][..]));
    assert_eq!(metadata.xml_namespace.as_deref(), Some("https://x/"));
    assert_eq!(metadata.global_endpoint.as_deref(), Some("x.amazonaws.com"));
    assert_eq!(metadata.checksum_format.as_deref(), Some("md5"));
    let params = model.client_context_params();
    assert_eq!(params.len(), 1);
    assert_eq!((&*params[0].name, &*params[0].type_name), ("Accelerate", "boolean"));
    Ok(())
}

#[test]
fn operations_keep_model_order_and_their_traits() -> Result<()> {
    let model = fixture()?;
    let names: Vec<&str> = model.operations().iter().map(|op| &*op.name).collect();
    assert_eq!(names, ["Zeta", "Alpha"]);
    let zeta = model.operation("Zeta").ok_or("Zeta")?;
    assert_eq!(zeta.http.method.as_deref(), Some("POST"));
    assert_eq!(zeta.http.request_uri.as_deref(), Some("/"));
    assert_eq!(zeta.http.response_code, Some(200));
    assert_eq!(zeta.host_prefix.as_deref(), Some("{Name}."));
    let statics = &zeta.static_context_params;
    assert_eq!(statics.len(), 2);
    assert_eq!((&*statics[0].0, &statics[0].1), ("UseThing", &Value::Bool(true)));
    assert_eq!((&*statics[1].0, &statics[1].1), ("Region", &Value::String("r".into())));
    let contexts = &zeta.operation_context_params;
    assert_eq!(contexts.len(), 1);
    assert_eq!((&*contexts[0].0, &*contexts[0].1), ("Names", "Items[*].Name"));
    let auth = zeta.auth.as_deref().ok_or("auth")?;
    assert_eq!(auth, [Box::from("aws.auth#sigv4"), Box::from("smithy.api#noAuth")]);
    assert_eq!(zeta.auth_type.as_deref(), Some("v4"));
    assert!(zeta.unsigned_payload && zeta.deprecated && !zeta.endpoint_operation);
    assert_eq!(zeta.endpoint_discovery, Some(EndpointDiscovery { required: true }));
    let checksum = zeta.http_checksum.as_ref().ok_or("httpChecksum")?;
    assert_eq!(checksum.request_algorithm_member.as_deref(), Some("Algo"));
    assert!(checksum.request_checksum_required);
    assert_eq!(checksum.request_validation_mode_member.as_deref(), Some("Mode"));
    assert_eq!(&*checksum.response_algorithms, [Box::from("CRC32"), Box::from("SHA256")]);
    let compression = zeta.request_compression.as_deref().ok_or("requestcompression")?;
    assert_eq!(compression, [Box::from("gzip")]);
    let alpha = model.operation("Alpha").ok_or("Alpha")?;
    assert_eq!(alpha.wire_name.as_deref(), Some("AlphaWire"));
    assert!(alpha.endpoint_operation && alpha.http_checksum_required);
    assert_eq!(alpha.endpoint_discovery, Some(EndpointDiscovery { required: false }));
    assert!(alpha.input.is_none() && alpha.output.is_none() && alpha.errors.is_empty());
    assert!(model.operation("Missing").is_none());
    Ok(())
}

#[test]
fn members_keep_model_order_and_references_overlay_their_targets() -> Result<()> {
    let model = fixture()?;
    let zeta = model.operation("Zeta").ok_or("Zeta")?;
    let input = zeta.input.ok_or("input")?;
    assert_eq!(input, named(&model, "ZetaInput")?);
    let expected = [
        "Name", "Token", "Plain", "Doc", "Items", "Tags", "Header", "Query", "Num", "Ratio",
        "Body", "When", "Choice", "Stream", "Secret", "Next",
    ];
    assert_eq!(member_names(&model, input), expected);
    assert_eq!(&*model.shape(input).traits.required, [Box::from("Name"), Box::from("Token")]);
    let str_id = named(&model, "Str")?;
    assert_eq!(member(&model, input, "Plain")?, str_id);
    assert_eq!(member(&model, input, "Doc")?, named(&model, "Doc")?);
    let name = model.shape(member(&model, input, "Name")?);
    assert_eq!((&*name.name, &name.kind), ("Str", &ShapeKind::String));
    assert_eq!(name.traits.name.as_deref(), Some("name"));
    assert!(name.traits.is(Flag::HostLabel));
    assert_eq!(name.traits.context_param.as_deref(), Some("ResourceName"));
    assert_eq!(name.traits.min, Some(Bound::Integer(1)));
    assert_eq!(name.traits.max, Some(Bound::Integer(64)));
    assert_eq!(name.traits.pattern.as_deref(), Some("^a+$"));
    assert_eq!(&*name.traits.enum_values, [Box::from("a"), Box::from("aa")]);
    assert!(!model.shape(str_id).traits.is(Flag::HostLabel));
    assert!(model.shape(str_id).traits.name.is_none());
    let token = model.shape(member(&model, input, "Token")?);
    assert!(token.traits.is(Flag::IdempotencyToken) && token.traits.min.is_some());
    Ok(())
}

#[test]
fn every_carried_trait_is_resolved() -> Result<()> {
    let model = fixture()?;
    let input = named(&model, "ZetaInput")?;
    let traits = |member_name: &str| -> Result<&Traits> {
        Ok(&model.shape(member(&model, input, member_name)?).traits)
    };
    let header = traits("Header")?;
    assert_eq!(header.location.as_deref(), Some("header"));
    assert_eq!(header.name.as_deref(), Some("X-H"));
    assert!(header.is(Flag::JsonValue));
    let query = traits("Query")?;
    assert_eq!(query.query_name.as_deref(), Some("q"));
    assert!(query.is(Flag::XmlAttribute));
    assert_eq!(traits("Ratio")?.min, Some(Bound::Float(0.5)));
    assert_eq!(traits("Ratio")?.max.map(|max| max.to_string()).as_deref(), Some("1000.0"));
    assert_eq!(traits("Num")?.min.map(|min| min.to_string()).as_deref(), Some("-5"));
    assert!(traits("Body")?.is(Flag::Streaming) && traits("Body")?.is(Flag::RequiresLength));
    assert!(!model.shape(named(&model, "Body")?).traits.is(Flag::Streaming));
    assert_eq!(traits("When")?.timestamp_format.as_deref(), Some("iso8601"));
    assert!(traits("Choice")?.is(Flag::Union) && traits("Doc")?.is(Flag::Document));
    assert!(traits("Stream")?.is(Flag::EventStream));
    assert!(traits("Secret")?.is(Flag::Deprecated) && traits("Secret")?.is(Flag::Sensitive));
    let next = traits("Next")?;
    let namespace = next.xml_namespace.as_ref().ok_or("xmlNamespace")?;
    assert_eq!(namespace.uri.as_deref(), Some("https://n/"));
    assert_eq!(namespace.prefix.as_deref(), Some("n"));
    let items = named(&model, "Items")?;
    assert!(model.shape(items).traits.is(Flag::Flattened));
    assert_eq!(model.shape(items).traits.min, Some(Bound::Integer(0)));
    let ShapeKind::List { member: item } = model.shape(items).kind else {
        return Err("Items is not a list".into());
    };
    assert_eq!(model.shape(item).traits.name.as_deref(), Some("item"));
    let ShapeKind::Map { key, value } = model.shape(named(&model, "Tags")?).kind else {
        return Err("Tags is not a map".into());
    };
    assert_eq!(key, named(&model, "Str")?);
    assert_eq!(model.shape(value).traits.name.as_deref(), Some("v"));
    let event = named(&model, "Ev")?;
    assert!(model.shape(event).traits.is(Flag::Event));
    assert!(model.shape(member(&model, event, "H")?).traits.is(Flag::EventHeader));
    assert!(model.shape(member(&model, event, "P")?).traits.is(Flag::EventPayload));
    let endpoint = named(&model, "Endpoint")?;
    assert!(model.shape(endpoint).traits.is(Flag::EndpointDiscoveryId));
    Ok(())
}

#[test]
fn recursive_shapes_and_overlaid_structures_share_their_members() -> Result<()> {
    let model = fixture()?;
    let node = named(&model, "Node")?;
    let next = member(&model, node, "Next")?;
    assert_ne!(next, node);
    assert_eq!(model.shape(next).traits.name.as_deref(), Some("n"));
    assert_eq!(member_names(&model, next), ["Next", "Id"]);
    assert_eq!(member(&model, next, "Next")?, next);
    let input = named(&model, "ZetaInput")?;
    let overlaid = member(&model, input, "Next")?;
    assert_eq!(model.shape(overlaid).traits.name.as_deref(), Some("next"));
    assert_eq!(member_names(&model, overlaid), ["Next", "Id"]);
    Ok(())
}

#[test]
fn outputs_errors_and_kinds_resolve_as_botocore_does() -> Result<()> {
    let model = fixture()?;
    let zeta = model.operation("Zeta").ok_or("Zeta")?;
    let output = model.shape(zeta.output.ok_or("output")?);
    assert_eq!(&*output.name, "ZetaOutput");
    assert_eq!(output.traits.result_wrapper.as_deref(), Some("ZetaResult"));
    assert_eq!(output.traits.payload.as_deref(), Some("Id"));
    let named_output = model.shape(named(&model, "ZetaOutput")?);
    assert!(named_output.traits.result_wrapper.is_none());
    let boom = named(&model, "Boom")?;
    let plain = named(&model, "Plain")?;
    assert_eq!(zeta.errors[0], boom);
    assert!(!model.shape(zeta.errors[1]).traits.is(Flag::Exception));
    let error = model.shape(boom).traits.error.as_ref().ok_or("error")?;
    assert_eq!(error.code.as_deref(), Some("BoomCode"));
    assert_eq!((error.http_status_code, error.sender_fault), (Some(400), true));
    assert_eq!(model.shape(boom).traits.retryable, Some(Retryable { throttling: true }));
    assert_eq!(model.error_shapes().collect::<Vec<_>>(), [boom, plain]);
    assert_eq!(model.error_code(boom), Some("BoomCode"));
    assert_eq!(model.error_code(plain), Some("Plain"));
    assert_eq!(model.error_code(named(&model, "Str")?), None);
    assert_eq!(model.shape_for_error_code("BoomCode"), Some(boom));
    assert_eq!(model.shape_for_error_code("Boom"), None);
    for (name, kind) in [
        ("Big", ShapeKind::Other("bigInteger".into())),
        ("Flags", ShapeKind::Boolean),
        ("Count", ShapeKind::Long),
        ("Share", ShapeKind::Float),
        ("Ratio", ShapeKind::Double),
        ("Num", ShapeKind::Integer),
        ("Body", ShapeKind::Blob),
        ("When", ShapeKind::Timestamp),
    ] {
        assert_eq!(model.shape(named(&model, name)?).kind, kind, "{name}");
    }
    Ok(())
}

#[test]
fn malformed_models_name_what_botocore_would_raise() -> Result<()> {
    let wrap = |shapes: &str| {
        let text = format!(
            r#"{{"metadata":{{}},"operations":{{"Op":{{"input":{{"shape":"In"}}}}}},"shapes":{{{shapes}}}}}"#
        );
        Model::from_json(text.as_bytes())
    };
    let cases: [(&str, fn(&ModelError) -> bool); 5] = [
        (r#""In":{"type":"structure","members":{"A":{"shape":"Gone"}}}"#, |e| {
            matches!(e, ModelError::NoShapeFound(name) if &**name == "Gone")
        }),
        (r#""In":{"members":{}}"#, |e| {
            matches!(e, ModelError::InvalidShape(name) if &**name == "In")
        }),
        (r#""In":{"type":"structure","members":{"A":{"location":"header"}}}"#, |e| {
            matches!(e, ModelError::InvalidShapeReference(_))
        }),
        (r#""In":{"type":"string","min":"one"}"#, |e| {
            matches!(e, ModelError::InvalidTrait { shape, name } if &**shape == "In" && *name == "min")
        }),
        (r#""Other":{"type":"string"}"#, |e| {
            matches!(e, ModelError::NoShapeFound(name) if &**name == "In")
        }),
    ];
    for (shapes, expected) in cases {
        let error = wrap(shapes).err().ok_or_else(|| format!("{shapes} loaded"))?;
        assert!(expected(&error), "{shapes}: {error:?}");
    }
    assert!(matches!(Model::from_json(b"{"), Err(ModelError::Json(_))));
    assert!(matches!(Model::from_json(b"[]"), Err(ModelError::NotAnObject)));
    Ok(())
}

#[test]
fn every_service_in_the_repository_loads_into_the_arena() -> Result<()> {
    let source = DirSource::new(testing::data_dir());
    let services = source.services(Kind::Service)?;
    let directories = std::fs::read_dir(testing::data_dir())?
        .filter(|entry| entry.as_ref().is_ok_and(|entry| entry.path().is_dir()))
        .count();
    assert_eq!(services.len(), directories);
    let mut operations = 0;
    for service in &services {
        let model = Model::load(&source, service).map_err(|e| format!("{service}: {e}"))?;
        operations += model.operations().len();
        for operation in model.operations() {
            for shape in operation.input.iter().chain(&operation.output) {
                assert!(matches!(model.shape(*shape).kind, ShapeKind::Structure { .. }));
            }
        }
    }
    assert!(operations > services.len());
    Ok(())
}

/// Prints parse and arena time per service. Run in release mode:
/// `cargo xtask test -p model --release --filter load_timings -- --ignored --nocapture`.
#[test]
#[ignore = "a timing report, not a check"]
fn load_timings() -> Result<()> {
    let source = DirSource::new(testing::data_dir());
    let mut report = String::from("service    bytes  parse_ms  arena_ms  ops/shapes\n");
    for service in ["sts", "dynamodb", "s3", "ec2"] {
        let version = crate::source::latest_version(&source, service, Kind::Service)?;
        let bytes = source.read(&format!("{service}/{version}/service-2"))?.ok_or(service)?;
        let (mut parse, mut arena) = (std::time::Duration::MAX, std::time::Duration::MAX);
        let mut counts = (0, 0);
        for _ in 0..7 {
            let started = std::time::Instant::now();
            let json: serde_json::Value = serde_json::from_slice(&bytes)?;
            let parsed = started.elapsed();
            let model = Model::from_value(&json)?;
            arena = arena.min(started.elapsed() - parsed);
            parse = parse.min(parsed);
            counts = (model.operations().len(), model.shape_count());
        }
        let line = format!(
            "{service:<9} {:>8} {:>9.2} {:>9.2}  {}/{}\n",
            bytes.len(),
            parse.as_secs_f64() * 1e3,
            arena.as_secs_f64() * 1e3,
            counts.0,
            counts.1
        );
        report.push_str(&line);
    }
    std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes())?;
    Ok(())
}
