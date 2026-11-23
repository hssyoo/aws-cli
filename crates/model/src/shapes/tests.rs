use super::*;
use crate::source::{DirSource, Kind, Source};
use crate::testing::{self, Checks, Result};
use crate::value::Value;

fn fixture() -> Result<Model> {
    Ok(Model::from_json(include_bytes!("fixture.json"))?)
}

fn named(model: &Model, name: &str) -> Result<ShapeId> {
    Ok(model
        .shape_named(name)
        .ok_or_else(|| format!("no shape {name}"))?)
}

fn member(model: &Model, shape: ShapeId, name: &str) -> Result<ShapeId> {
    let found = model
        .members(shape)
        .iter()
        .find(|member| &*member.name == name);
    Ok(found.ok_or_else(|| format!("no member {name}"))?.shape)
}

fn member_names(model: &Model, shape: ShapeId) -> Vec<&str> {
    model
        .members(shape)
        .iter()
        .map(|member| &*member.name)
        .collect()
}

fn text(value: &str) -> Option<Box<str>> {
    Some(value.into())
}

fn texts(values: &[&str]) -> Box<[Box<str>]> {
    values.iter().map(|value| Box::from(*value)).collect()
}

/// Keys no call reads are neither stored nor uninterpreted: the fixture's
/// `serviceFullName`, `globalEndpoint`, `checksumFormat`, an operation's
/// `deprecated` and `http.responseCode`.
#[test]
fn keys_no_call_reads_are_ignored() -> Result<()> {
    assert_eq!(fixture()?.uninterpreted(), &[]);
    Ok(())
}

#[test]
fn metadata_and_client_context_parameters_are_read() -> Result<()> {
    let model = fixture()?;
    let metadata = model.metadata();
    let mut checks = Checks::default();
    for (label, actual, expected) in [
        ("apiVersion", &metadata.api_version, "2020-01-01"),
        ("endpointPrefix", &metadata.endpoint_prefix, "x"),
        ("protocol", &metadata.protocol, "json"),
        ("jsonVersion", &metadata.json_version, "1.1"),
        ("targetPrefix", &metadata.target_prefix, "X"),
        ("serviceId", &metadata.service_id, "X Svc"),
        ("signatureVersion", &metadata.signature_version, "v4"),
        ("signingName", &metadata.signing_name, "xs"),
        ("uid", &metadata.uid, "x-2020-01-01"),
        ("xmlNamespace", &metadata.xml_namespace, "https://x/"),
    ] {
        checks.eq(label, actual, &text(expected));
    }
    checks.eq(
        "protocols",
        &metadata.protocols,
        &Some(texts(&["json", "query"])),
    );
    checks.eq("auth", &metadata.auth, &Some(texts(&["aws.auth#sigv4"])));
    checks.is("awsQueryCompatible", metadata.aws_query_compatible);
    let params = model.client_context_params();
    let expected = [ClientContextParam {
        name: "Accelerate".into(),
        type_name: "boolean".into(),
    }];
    checks.eq("clientContextParams", params, &expected[..]);
    checks.finish()
}

#[test]
fn operations_keep_model_order_and_their_traits() -> Result<()> {
    let model = fixture()?;
    let names: Vec<&str> = model.operations().iter().map(|op| &*op.name).collect();
    let zeta = model.operation("Zeta").ok_or("Zeta")?;
    let mut checks = Checks::default();
    checks.eq("order", &names[..], &["Zeta", "Alpha"][..]);
    let http = Http {
        method: text("POST"),
        request_uri: text("/"),
    };
    checks.eq("http", &zeta.http, &http);
    checks.eq("hostPrefix", &zeta.host_prefix, &text("{Name}."));
    let statics: Box<[(Box<str>, Value)]> = Box::new([
        ("UseThing".into(), Value::Bool(true)),
        ("Region".into(), Value::String("r".into())),
    ]);
    checks.eq("staticContextParams", &zeta.static_context_params, &statics);
    let contexts: Box<[(Box<str>, Box<str>)]> =
        Box::new([("Names".into(), "Items[*].Name".into())]);
    checks.eq(
        "operationContextParams",
        &zeta.operation_context_params,
        &contexts,
    );
    checks.eq(
        "auth",
        &zeta.auth,
        &Some(texts(&["aws.auth#sigv4", "smithy.api#noAuth"])),
    );
    checks.eq("authtype", &zeta.auth_type, &text("v4"));
    checks.is("unsignedPayload", zeta.unsigned_payload);
    checks.is("not endpointoperation", !zeta.endpoint_operation);
    checks.eq(
        "endpointdiscovery",
        &zeta.endpoint_discovery,
        &Some(EndpointDiscovery { required: true }),
    );
    let checksum = HttpChecksum {
        request_algorithm_member: text("Algo"),
        request_checksum_required: true,
        request_validation_mode_member: text("Mode"),
        response_algorithms: texts(&["CRC32", "SHA256"]),
    };
    checks.eq("httpChecksum", &zeta.http_checksum, &Some(checksum));
    checks.eq(
        "requestcompression",
        &zeta.request_compression,
        &Some(texts(&["gzip"])),
    );
    checks.is("no Missing operation", model.operation("Missing").is_none());
    checks.finish()
}

#[test]
fn an_operation_without_io_reads_its_wire_name_and_flags() -> Result<()> {
    let model = fixture()?;
    let alpha = model.operation("Alpha").ok_or("Alpha")?;
    let mut checks = Checks::default();
    checks.eq("wire name", &alpha.wire_name, &text("AlphaWire"));
    checks.is("endpointoperation", alpha.endpoint_operation);
    checks.is("httpChecksumRequired", alpha.http_checksum_required);
    checks.eq(
        "endpointdiscovery",
        &alpha.endpoint_discovery,
        &Some(EndpointDiscovery { required: false }),
    );
    checks.eq("input", &alpha.input, &None);
    checks.eq("output", &alpha.output, &None);
    checks.is("no errors", alpha.errors.is_empty());
    checks.finish()
}

#[test]
fn members_keep_model_order_and_references_overlay_their_targets() -> Result<()> {
    let model = fixture()?;
    let input = model
        .operation("Zeta")
        .and_then(|op| op.input)
        .ok_or("input")?;
    let str_id = named(&model, "Str")?;
    let name = model.shape(member(&model, input, "Name")?);
    let token = model.shape(member(&model, input, "Token")?);
    let mut checks = Checks::default();
    checks.eq("input", &input, &named(&model, "ZetaInput")?);
    let expected = [
        "Name", "Token", "Plain", "Doc", "Items", "Tags", "Header", "Query", "Num", "Ratio",
        "Body", "When", "Choice", "Stream", "Secret", "Next",
    ];
    checks.eq("members", &member_names(&model, input)[..], &expected[..]);
    checks.eq(
        "required",
        &*model.shape(input).traits.required,
        &*texts(&["Name", "Token"]),
    );
    checks.eq("Plain is Str", &member(&model, input, "Plain")?, &str_id);
    checks.eq(
        "Doc is Doc",
        &member(&model, input, "Doc")?,
        &named(&model, "Doc")?,
    );
    checks.eq(
        "Name shape",
        &(&*name.name, &name.kind),
        &("Str", &ShapeKind::String),
    );
    checks.eq("Name locationName", &name.traits.name, &text("name"));
    checks.is("Name hostLabel", name.traits.is(Flag::HostLabel));
    checks.eq(
        "Name contextParam",
        &name.traits.context_param,
        &text("ResourceName"),
    );
    checks.eq("Name min", &name.traits.min, &Some(Bound::Integer(1)));
    checks.eq("Name max", &name.traits.max, &Some(Bound::Integer(64)));
    checks.eq("Name pattern", &name.traits.pattern, &text("^a+$"));
    checks.eq(
        "Name enum",
        &*name.traits.enum_values,
        &*texts(&["a", "aa"]),
    );
    checks.is(
        "Str keeps no hostLabel",
        !model.shape(str_id).traits.is(Flag::HostLabel),
    );
    checks.eq(
        "Str has no locationName",
        &model.shape(str_id).traits.name,
        &None,
    );
    checks.is(
        "Token idempotencyToken",
        token.traits.is(Flag::IdempotencyToken),
    );
    checks.eq("Token min", &token.traits.min, &Some(Bound::Integer(1)));
    checks.finish()
}

#[test]
fn member_traits_are_resolved() -> Result<()> {
    let model = fixture()?;
    let input = named(&model, "ZetaInput")?;
    let traits = |member_name: &str| -> Result<&Traits> {
        Ok(&model.shape(member(&model, input, member_name)?).traits)
    };
    let mut checks = Checks::default();
    let header = traits("Header")?;
    checks.eq("Header location", &header.location, &text("header"));
    checks.eq("Header name", &header.name, &text("X-H"));
    checks.is("Header jsonvalue", header.is(Flag::JsonValue));
    let query = traits("Query")?;
    checks.eq("Query queryName", &query.query_name, &text("q"));
    checks.is("Query xmlAttribute", query.is(Flag::XmlAttribute));
    checks.eq("Ratio min", &traits("Ratio")?.min, &Some(Bound::Float(0.5)));
    let ratio_max = traits("Ratio")?.max.map(|max| max.to_string());
    checks.eq("Ratio max text", &ratio_max, &Some("1000.0".to_owned()));
    let num_min = traits("Num")?.min.map(|min| min.to_string());
    checks.eq("Num min text", &num_min, &Some("-5".to_owned()));
    checks.is("Body streaming", traits("Body")?.is(Flag::Streaming));
    checks.is(
        "Body requiresLength",
        traits("Body")?.is(Flag::RequiresLength),
    );
    checks.is(
        "Body shape not streaming",
        !model
            .shape(named(&model, "Body")?)
            .traits
            .is(Flag::Streaming),
    );
    checks.eq(
        "When timestampFormat",
        &traits("When")?.timestamp_format,
        &text("iso8601"),
    );
    checks.is("Choice union", traits("Choice")?.is(Flag::Union));
    checks.is("Doc document", traits("Doc")?.is(Flag::Document));
    checks.is(
        "Stream eventstream",
        traits("Stream")?.is(Flag::EventStream),
    );
    checks.is("Secret deprecated", traits("Secret")?.is(Flag::Deprecated));
    checks.is("Secret sensitive", traits("Secret")?.is(Flag::Sensitive));
    let namespace = XmlNamespace {
        uri: text("https://n/"),
        prefix: text("n"),
    };
    checks.eq(
        "Next xmlNamespace",
        &traits("Next")?.xml_namespace,
        &Some(namespace),
    );
    checks.finish()
}

#[test]
fn list_map_and_event_traits_are_resolved() -> Result<()> {
    let model = fixture()?;
    let items = named(&model, "Items")?;
    let ShapeKind::List { member: item } = model.shape(items).kind else {
        return Err("Items is not a list".into());
    };
    let ShapeKind::Map { key, value } = model.shape(named(&model, "Tags")?).kind else {
        return Err("Tags is not a map".into());
    };
    let event = named(&model, "Ev")?;
    let mut checks = Checks::default();
    checks.is(
        "Items flattened",
        model.shape(items).traits.is(Flag::Flattened),
    );
    checks.eq(
        "Items min",
        &model.shape(items).traits.min,
        &Some(Bound::Integer(0)),
    );
    checks.eq(
        "item locationName",
        &model.shape(item).traits.name,
        &text("item"),
    );
    checks.eq("map key", &key, &named(&model, "Str")?);
    checks.eq(
        "map value locationName",
        &model.shape(value).traits.name,
        &text("v"),
    );
    checks.is("Ev event", model.shape(event).traits.is(Flag::Event));
    checks.is(
        "H eventheader",
        model
            .shape(member(&model, event, "H")?)
            .traits
            .is(Flag::EventHeader),
    );
    checks.is(
        "P eventpayload",
        model
            .shape(member(&model, event, "P")?)
            .traits
            .is(Flag::EventPayload),
    );
    let endpoint = model.shape(named(&model, "Endpoint")?);
    checks.is(
        "endpointdiscoveryid",
        endpoint.traits.is(Flag::EndpointDiscoveryId),
    );
    checks.finish()
}

#[test]
fn recursive_shapes_and_overlaid_structures_share_their_members() -> Result<()> {
    let model = fixture()?;
    let node = named(&model, "Node")?;
    let next = member(&model, node, "Next")?;
    let input = named(&model, "ZetaInput")?;
    let overlaid = member(&model, input, "Next")?;
    let mut checks = Checks::default();
    checks.is("derived shape", next != node);
    checks.eq(
        "Next locationName",
        &model.shape(next).traits.name,
        &text("n"),
    );
    checks.eq(
        "Next members",
        &member_names(&model, next)[..],
        &["Next", "Id"][..],
    );
    checks.eq("Next is recursive", &member(&model, next, "Next")?, &next);
    checks.eq(
        "overlaid locationName",
        &model.shape(overlaid).traits.name,
        &text("next"),
    );
    checks.eq(
        "overlaid members",
        &member_names(&model, overlaid)[..],
        &["Next", "Id"][..],
    );
    checks.finish()
}

#[test]
fn outputs_and_errors_resolve_as_botocore_does() -> Result<()> {
    let model = fixture()?;
    let zeta = model.operation("Zeta").ok_or("Zeta")?;
    let output = model.shape(zeta.output.ok_or("output")?);
    let (boom, plain) = (named(&model, "Boom")?, named(&model, "Plain")?);
    let mut checks = Checks::default();
    checks.eq("output name", &*output.name, "ZetaOutput");
    checks.eq(
        "resultWrapper",
        &output.traits.result_wrapper,
        &text("ZetaResult"),
    );
    checks.eq("payload", &output.traits.payload, &text("Id"));
    let named_output = model.shape(named(&model, "ZetaOutput")?);
    checks.eq(
        "named output has no wrapper",
        &named_output.traits.result_wrapper,
        &None,
    );
    checks.eq("first error", &zeta.errors[0], &boom);
    checks.is(
        "overlaid exception",
        !model.shape(zeta.errors[1]).traits.is(Flag::Exception),
    );
    let error = ErrorTrait {
        code: text("BoomCode"),
        http_status_code: Some(400),
        sender_fault: true,
    };
    checks.eq("error", &model.shape(boom).traits.error, &Some(error));
    checks.eq(
        "retryable",
        &model.shape(boom).traits.retryable,
        &Some(Retryable { throttling: true }),
    );
    checks.eq(
        "error shapes",
        &model.error_shapes().collect::<Vec<_>>()[..],
        &[boom, plain][..],
    );
    checks.eq("Boom code", &model.error_code(boom), &Some("BoomCode"));
    checks.eq("Plain code", &model.error_code(plain), &Some("Plain"));
    checks.eq("Str code", &model.error_code(named(&model, "Str")?), &None);
    checks.eq(
        "by code",
        &model.shape_for_error_code("BoomCode"),
        &Some(boom),
    );
    checks.eq("by name", &model.shape_for_error_code("Boom"), &None);
    checks.finish()
}

#[test]
fn every_shape_type_has_its_kind() -> Result<()> {
    let model = fixture()?;
    let mut checks = Checks::default();
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
        checks.eq(name, &model.shape(named(&model, name)?).kind, &kind);
    }
    checks.finish()
}

#[test]
fn malformed_models_name_what_botocore_would_raise() -> Result<()> {
    let wrap = |shapes: &str| {
        let text = format!(
            concat!(
                r#"{{"metadata":{{}},"operations":{{"Op":{{"input":{{"shape":"In"}}}}}},"#,
                r#""shapes":{{{shapes}}}}}"#
            ),
            shapes = shapes,
        );
        Model::from_json(text.as_bytes())
    };
    type Expect = fn(&ModelError) -> bool;
    let cases: [(&str, Expect); 5] = [
        (
            r#""In":{"type":"structure","members":{"A":{"shape":"Gone"}}}"#,
            |e| matches!(e, ModelError::NoShapeFound(name) if &**name == "Gone"),
        ),
        (
            r#""In":{"members":{}}"#,
            |e| matches!(e, ModelError::InvalidShape(name) if &**name == "In"),
        ),
        (
            r#""In":{"type":"structure","members":{"A":{"location":"header"}}}"#,
            |e| matches!(e, ModelError::InvalidShapeReference(_)),
        ),
        (r#""In":{"type":"string","min":"one"}"#, |e| {
            let ModelError::InvalidTrait { shape, name } = e else {
                return false;
            };
            &**shape == "In" && *name == "min"
        }),
        (
            r#""Other":{"type":"string"}"#,
            |e| matches!(e, ModelError::NoShapeFound(name) if &**name == "In"),
        ),
    ];
    for (shapes, expected) in cases {
        let error = wrap(shapes)
            .err()
            .ok_or_else(|| format!("{shapes} loaded"))?;
        assert!(expected(&error), "{shapes}: {error:?}");
    }
    assert!(matches!(Model::from_json(b"{"), Err(ModelError::Json(_))));
    assert!(matches!(
        Model::from_json(b"[]"),
        Err(ModelError::NotAnObject)
    ));
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
                assert!(matches!(
                    model.shape(*shape).kind,
                    ShapeKind::Structure { .. }
                ));
            }
        }
    }
    assert!(operations > services.len());
    Ok(())
}

/// Prints parse and arena time per service, best of seven. Run in release
/// mode (it is the package's only ignored test):
/// `RUST_TEST_NOCAPTURE=1 cargo xtask test -p model --profile release --filter --ignored`.
#[test]
#[ignore = "a timing report, not a check"]
fn load_timings() -> Result<()> {
    let source = DirSource::new(testing::data_dir());
    let mut report = String::from("service    bytes  parse_ms  arena_ms  ops/shapes\n");
    for service in ["sts", "dynamodb", "s3", "ec2"] {
        let version = crate::source::latest_version(&source, service, Kind::Service)?;
        let bytes = source
            .read(&format!("{service}/{version}/service-2"))?
            .ok_or(service)?;
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

/// `xmlNamespace` written as the URI alone.
#[test]
fn a_namespace_may_be_written_as_its_uri() -> Result<()> {
    let model = Model::from_json(
        br#"{"shapes":{"L":{"type":"list","member":{"shape":"S"},"xmlNamespace":"https://x/"},
        "S":{"type":"string"}}}"#,
    )?;
    let list = named(&model, "L")?;
    let namespace = model
        .shape(list)
        .traits
        .xml_namespace
        .clone()
        .ok_or("namespace")?;
    assert_eq!(namespace.uri.as_deref(), Some("https://x/"));
    assert_eq!(namespace.prefix, None);
    Ok(())
}

#[test]
fn the_service_id_hyphenizes_as_botocore_s_does() -> Result<()> {
    let model = fixture()?;
    let mut checks = Checks::default();
    checks.eq(
        "fixture",
        &model.metadata().hyphenized_service_id(),
        &Some("x-svc".to_owned()),
    );
    let mut metadata = Metadata::default();
    checks.eq("absent", &metadata.hyphenized_service_id(), &None);
    metadata.service_id = Some("Lex Runtime V2".into());
    checks.eq(
        "words",
        &metadata.hyphenized_service_id(),
        &Some("lex-runtime-v2".to_owned()),
    );
    checks.finish()
}
