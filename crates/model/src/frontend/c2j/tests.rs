//! What the C2J front-end reads of a description, through the schema: the
//! fixture's every key (as the arena's own tests read it before), and the
//! descriptions botocore ships, fakes and the protocol suite build or are
//! refused without a panic.
use crate::schema::{
    Bound, EndpointDiscovery, Flag, HttpChecksum, Retryable, Schema, SchemaSet, ShapeType,
    XmlNamespaceView, traits,
};
use crate::shapes::Model;
use crate::source::{DirSource, Kind, Source};
use crate::testing::{self, Checks, Result};
use crate::value::Value;
use serde_json::{Value as Json, json};
use std::path::Path;

fn fixture() -> Result<Model> {
    Ok(Model::from_json(include_bytes!("fixture.json"))?)
}

fn named<'s>(set: &'s SchemaSet, name: &str) -> Result<Schema<'s>> {
    Ok(set
        .shape_named(name)
        .ok_or_else(|| format!("no shape {name}"))?)
}

fn member<'s>(shape: Schema<'s>, name: &str) -> Result<Schema<'s>> {
    Ok(shape
        .target()
        .member(name)
        .ok_or_else(|| format!("no member {name}"))?)
}

fn member_names(shape: Schema<'_>) -> Vec<&str> {
    shape
        .target()
        .members()
        .filter_map(Schema::member_name)
        .collect()
}

fn texts(values: &[&str]) -> Box<[Box<str>]> {
    values.iter().map(|value| Box::from(*value)).collect()
}

/// Python truthiness of a cold `c2j#` value, absent as false.
fn cold(schema: Schema<'_>, id: crate::schema::TraitId) -> bool {
    match schema.cold_trait(id.as_str()) {
        None | Some(Json::Null | Json::Bool(false)) => false,
        Some(_) => true,
    }
}

/// Keys no call reads are neither stored nor uninterpreted: the fixture's
/// `serviceFullName`, `globalEndpoint`, `checksumFormat`, an operation's
/// `deprecated` and `http.responseCode`.
#[test]
fn keys_no_call_reads_are_ignored() -> Result<()> {
    assert_eq!(fixture()?.schema().uninterpreted(), &[]);
    Ok(())
}

#[test]
fn metadata_and_client_context_parameters_are_read() -> Result<()> {
    let model = fixture()?;
    let service = model.schema().service();
    let metadata = model.metadata();
    let mut checks = Checks::default();
    for (label, actual, expected) in [
        ("apiVersion", service.version(), "2020-01-01"),
        ("endpointPrefix", service.endpoint_prefix(), "x"),
        ("serviceId", service.sdk_id(), "X Svc"),
        ("signingName", service.sigv4_name(), "xs"),
        ("xmlNamespace", service.xml_namespace(), "https://x/"),
        ("protocol", metadata.protocol.as_deref(), "json"),
        ("jsonVersion", metadata.json_version.as_deref(), "1.1"),
        ("targetPrefix", metadata.target_prefix.as_deref(), "X"),
        (
            "signatureVersion",
            metadata.signature_version.as_deref(),
            "v4",
        ),
        ("uid", metadata.uid.as_deref(), "x-2020-01-01"),
    ] {
        checks.eq(label, &actual, &Some(expected));
    }
    checks.eq(
        "protocols",
        &metadata.protocols,
        &Some(texts(&["json", "query"])),
    );
    let auth = service.auth().map(Iterator::collect::<Vec<_>>);
    checks.eq("auth", &auth, &Some(vec!["aws.auth#sigv4"]));
    checks.is("awsQueryCompatible", service.query_compatible());
    let params: Vec<(&str, &str)> = (service.client_context_params().iter())
        .map(|param| (&*param.name, &*param.type_name))
        .collect();
    checks.eq(
        "clientContextParams",
        &params,
        &vec![("Accelerate", "boolean")],
    );
    checks.finish()
}

#[test]
fn operations_keep_model_order_and_their_traits() -> Result<()> {
    let model = fixture()?;
    let service = model.schema().service();
    let names: Vec<&str> = service.operations().map(|op| op.name()).collect();
    let zeta = service.operation("Zeta").ok_or("Zeta")?;
    let mut checks = Checks::default();
    checks.eq("order", &names[..], &["Zeta", "Alpha"][..]);
    checks.eq(
        "http",
        &(zeta.http_method(), zeta.http_request_uri()),
        &(Some("POST"), Some("/")),
    );
    checks.eq("hostPrefix", &zeta.host_prefix(), &Some("{Name}."));
    let statics: Vec<(&str, &Value)> = zeta.static_context_params().collect();
    let (yes, region) = (Value::Bool(true), Value::String("r".into()));
    checks.eq(
        "staticContextParams",
        &statics,
        &vec![("UseThing", &yes), ("Region", &region)],
    );
    let contexts: Vec<(&str, &str)> = zeta.operation_context_params().collect();
    checks.eq(
        "operationContextParams",
        &contexts,
        &vec![("Names", "Items[*].Name")],
    );
    let auth = zeta.auth().map(Iterator::collect::<Vec<_>>);
    checks.eq(
        "auth",
        &auth,
        &Some(vec!["aws.auth#sigv4", "smithy.api#noAuth"]),
    );
    checks.eq("authtype", &zeta.auth_type(), &Some("v4"));
    checks.is("unsignedPayload", zeta.unsigned_payload());
    checks.is("not endpointoperation", !zeta.endpoint_operation());
    let required = Some(EndpointDiscovery { required: true });
    checks.eq("endpointdiscovery", &zeta.endpoint_discovery(), &required);
    let checksum = HttpChecksum {
        request_algorithm_member: Some("Algo".into()),
        request_checksum_required: true,
        request_validation_mode_member: Some("Mode".into()),
        response_algorithms: texts(&["CRC32", "SHA256"]),
    };
    checks.eq("httpChecksum", &zeta.http_checksum(), &Some(&checksum));
    let compression = zeta.request_compression().map(Iterator::collect::<Vec<_>>);
    checks.eq("requestcompression", &compression, &Some(vec!["gzip"]));
    checks.is(
        "no Missing operation",
        service.operation("Missing").is_none(),
    );
    checks.finish()
}

#[test]
fn an_operation_without_io_reads_its_wire_name_and_flags() -> Result<()> {
    let model = fixture()?;
    let alpha = model.schema().service().operation("Alpha").ok_or("Alpha")?;
    let mut checks = Checks::default();
    checks.eq("wire name", &alpha.wire_name(), &Some("AlphaWire"));
    checks.is("endpointoperation", alpha.endpoint_operation());
    checks.is("httpChecksumRequired", alpha.http_checksum_required());
    let optional = Some(EndpointDiscovery { required: false });
    checks.eq("endpointdiscovery", &alpha.endpoint_discovery(), &optional);
    checks.is("no input", alpha.input().is_none());
    checks.is("no output", alpha.output().is_none());
    checks.eq("no errors", &alpha.errors().len(), &0);
    checks.finish()
}

#[test]
fn members_keep_model_order_and_references_overlay_their_targets() -> Result<()> {
    let model = fixture()?;
    let set = model.schema();
    let zeta = set.service().operation("Zeta").ok_or("Zeta")?;
    let input = zeta.input().ok_or("input")?;
    let str_shape = named(set, "Str")?;
    let (name, token) = (member(input, "Name")?, member(input, "Token")?);
    let mut checks = Checks::default();
    checks.eq("input", &input.target(), &named(set, "ZetaInput")?);
    let expected = [
        "Name", "Token", "Plain", "Doc", "Items", "Tags", "Header", "Query", "Num", "Ratio",
        "Body", "When", "Choice", "Stream", "Secret", "Next",
    ];
    checks.eq("members", &member_names(input)[..], &expected[..]);
    checks.eq(
        "required",
        &input.required().collect::<Vec<_>>(),
        &vec!["Name", "Token"],
    );
    checks.eq(
        "Plain is Str",
        &member(input, "Plain")?.target(),
        &str_shape,
    );
    checks.is(
        "Plain has no own traits",
        !member(input, "Plain")?.has_own_traits(),
    );
    checks.eq(
        "Doc is Doc",
        &member(input, "Doc")?.target(),
        &named(set, "Doc")?,
    );
    checks.eq("Name target", &name.target().id().name(), &"Str");
    checks.eq("Name type", &name.shape_type(), &ShapeType::String);
    checks.eq("Name locationName", &name.location_name(), &Some("name"));
    checks.is("Name hostLabel", name.flag(Flag::HostLabel));
    checks.eq(
        "Name contextParam",
        &name.context_param(),
        &Some("ResourceName"),
    );
    checks.eq("Name min", &name.min(), &Some(Bound::Integer(1)));
    checks.eq("Name max", &name.max(), &Some(Bound::Integer(64)));
    checks.eq("Name pattern", &name.pattern(), &Some("^a+$"));
    checks.eq(
        "Name enum",
        &name.enum_values().collect::<Vec<_>>(),
        &vec!["a", "aa"],
    );
    checks.is("Str keeps no hostLabel", !str_shape.flag(Flag::HostLabel));
    checks.eq("Str has no locationName", &str_shape.location_name(), &None);
    checks.is("Token idempotencyToken", token.flag(Flag::IdempotencyToken));
    checks.eq("Token min", &token.min(), &Some(Bound::Integer(1)));
    checks.finish()
}

#[test]
fn member_traits_are_resolved() -> Result<()> {
    let model = fixture()?;
    let input = named(model.schema(), "ZetaInput")?;
    let of = |name: &str| member(input, name);
    let mut checks = Checks::default();
    let header = of("Header")?;
    checks.eq("Header location", &header.location(), &Some("header"));
    checks.eq("Header name", &header.location_name(), &Some("X-H"));
    checks.eq("Header jsonName", &header.json_name(), &Some("X-H"));
    checks.is("Header jsonvalue", header.flag(Flag::JsonValue));
    let query = of("Query")?;
    checks.eq("Query queryName", &query.ec2_query_name(), &Some("q"));
    checks.is("Query xmlAttribute", query.flag(Flag::XmlAttribute));
    checks.eq("Ratio min", &of("Ratio")?.min(), &Some(Bound::Float(0.5)));
    let ratio_max = of("Ratio")?.max().map(|max| max.to_string());
    checks.eq("Ratio max text", &ratio_max, &Some("1000.0".to_owned()));
    let num_min = of("Num")?.min().map(|min| min.to_string());
    checks.eq("Num min text", &num_min, &Some("-5".to_owned()));
    checks.is("Body streaming", of("Body")?.flag(Flag::Streaming));
    checks.is(
        "Body requiresLength",
        of("Body")?.flag(Flag::RequiresLength),
    );
    let body = named(model.schema(), "Body")?;
    checks.is("Body shape not streaming", !body.flag(Flag::Streaming));
    checks.eq(
        "When timestampFormat",
        &of("When")?.timestamp_format_text(),
        &Some("iso8601"),
    );
    checks.eq(
        "Choice union",
        &of("Choice")?.shape_type(),
        &ShapeType::Union,
    );
    checks.eq(
        "Doc document",
        &of("Doc")?.shape_type(),
        &ShapeType::Document,
    );
    checks.is("Stream eventstream", of("Stream")?.flag(Flag::EventStream));
    checks.is("Secret deprecated", of("Secret")?.flag(Flag::Deprecated));
    checks.is("Secret sensitive", of("Secret")?.flag(Flag::Sensitive));
    let namespace = XmlNamespaceView {
        uri: Some("https://n/"),
        prefix: Some("n"),
    };
    checks.eq(
        "Next xmlNamespace",
        &of("Next")?.xml_namespace(),
        &Some(namespace),
    );
    checks.finish()
}

#[test]
fn list_map_and_event_traits_are_resolved() -> Result<()> {
    let model = fixture()?;
    let set = model.schema();
    let items = named(set, "Items")?;
    let item = items.list_member().ok_or("Items is not a list")?;
    let tags = named(set, "Tags")?;
    let (key, value) = (tags.map_key(), tags.map_value());
    let (key, value) = key.zip(value).ok_or("Tags is not a map")?;
    let event = named(set, "Ev")?;
    let mut checks = Checks::default();
    checks.is("Items flattened", items.flag(Flag::XmlFlattened));
    checks.eq("Items min", &items.min(), &Some(Bound::Integer(0)));
    checks.eq("item locationName", &item.location_name(), &Some("item"));
    checks.eq("map key", &key.target(), &named(set, "Str")?);
    checks.eq("map value locationName", &value.location_name(), &Some("v"));
    checks.is("Ev event", cold(event, traits::EVENT));
    checks.is("H eventheader", member(event, "H")?.flag(Flag::EventHeader));
    checks.is(
        "P eventpayload",
        member(event, "P")?.flag(Flag::EventPayload),
    );
    let endpoint = named(set, "Endpoint")?;
    checks.is(
        "endpointdiscoveryid",
        endpoint.flag(Flag::EndpointDiscoveryId),
    );
    checks.finish()
}

#[test]
fn recursive_shapes_and_overlaid_structures_share_their_members() -> Result<()> {
    let model = fixture()?;
    let set = model.schema();
    let node = named(set, "Node")?;
    let next = member(node, "Next")?;
    let overlaid = member(named(set, "ZetaInput")?, "Next")?;
    let mut checks = Checks::default();
    checks.is("a member of its own", next != node && next.target() == node);
    checks.eq("Next locationName", &next.location_name(), &Some("n"));
    checks.eq("Next members", &member_names(next)[..], &["Next", "Id"][..]);
    checks.eq("Next is recursive", &member(next, "Next")?, &next);
    checks.eq(
        "overlaid locationName",
        &overlaid.location_name(),
        &Some("next"),
    );
    checks.eq(
        "overlaid members",
        &member_names(overlaid)[..],
        &["Next", "Id"][..],
    );
    checks.finish()
}

#[test]
fn outputs_and_errors_resolve_as_botocore_does() -> Result<()> {
    let model = fixture()?;
    let set = model.schema();
    let zeta = set.service().operation("Zeta").ok_or("Zeta")?;
    let output = zeta.output().ok_or("output")?;
    let (boom, plain) = (named(set, "Boom")?, named(set, "Plain")?);
    let errors: Vec<Schema<'_>> = zeta.errors().collect();
    let registry = set.registry();
    let mut checks = Checks::default();
    checks.eq("output name", &output.target().id().name(), &"ZetaOutput");
    checks.eq(
        "resultWrapper",
        &output.result_wrapper(),
        &Some("ZetaResult"),
    );
    checks.eq("payload", &output.payload(), &Some("Id"));
    let named_output = named(set, "ZetaOutput")?;
    checks.eq(
        "named output has no wrapper",
        &named_output.result_wrapper(),
        &None,
    );
    checks.eq("first error", &errors[0].target(), &boom);
    checks.is("overlaid exception", !errors[1].flag(Flag::Error));
    let error = boom.error().ok_or("error")?;
    checks.eq(
        "error",
        &(error.code, error.http_status, error.sender_fault),
        &(Some("BoomCode"), Some(400), true),
    );
    checks.eq(
        "retryable",
        &boom.retryable(),
        &Some(Retryable { throttling: true }),
    );
    let shapes: Vec<Schema<'_>> = registry.error_shapes().collect();
    checks.eq("error shapes", &shapes, &vec![boom, plain]);
    checks.eq("Boom code", &registry.error_code(boom), &Some("BoomCode"));
    checks.eq("Plain code", &registry.error_code(plain), &Some("Plain"));
    checks.eq("Str code", &registry.error_code(named(set, "Str")?), &None);
    checks.eq(
        "by code",
        &registry.entry_for_error_code("BoomCode"),
        &Some(boom),
    );
    checks.eq("by name", &registry.entry_for_error_code("Boom"), &None);
    checks.finish()
}

#[test]
fn every_shape_type_has_its_type() -> Result<()> {
    let model = fixture()?;
    let mut checks = Checks::default();
    for (name, shape_type, type_name) in [
        ("Big", ShapeType::Unknown, "bigInteger"),
        ("Flags", ShapeType::Boolean, "boolean"),
        ("Count", ShapeType::Long, "long"),
        ("Share", ShapeType::Float, "float"),
        ("Ratio", ShapeType::Double, "double"),
        ("Num", ShapeType::Integer, "integer"),
        ("Body", ShapeType::Blob, "blob"),
        ("When", ShapeType::Timestamp, "timestamp"),
    ] {
        let shape = named(model.schema(), name)?;
        checks.eq(
            name,
            &(shape.shape_type(), shape.type_name()),
            &(shape_type, type_name),
        );
    }
    checks.finish()
}

/// `xmlNamespace` written as the URI alone.
#[test]
fn a_namespace_may_be_written_as_its_uri() -> Result<()> {
    let model = Model::from_json(
        br#"{"shapes":{"L":{"type":"list","member":{"shape":"S"},"xmlNamespace":"https://x/"},
        "S":{"type":"string"}}}"#,
    )?;
    let namespace = named(model.schema(), "L")?
        .xml_namespace()
        .ok_or("namespace")?;
    assert_eq!(namespace.uri, Some("https://x/"));
    assert_eq!(namespace.prefix, None);
    Ok(())
}

/// References that change a structure target's `document` or `union`
/// are recorded, after the reference's own unknown keys.
#[test]
fn a_reference_that_changes_what_a_structure_is_is_recorded() -> Result<()> {
    let model = json!({"shapes": {
        "S": {"type": "structure", "members": {
            "D": {"shape": "S", "document": true, "newKey": 1},
            "U": {"shape": "Doc", "union": true, "document": false}}},
        "Doc": {"type": "structure", "members": {}, "document": true},
        "Str": {"type": "string", "document": true},
        "L": {"type": "list", "member": {"shape": "Str", "document": false}}}});
    let model = Model::from_value(&model)?;
    let keys: Vec<String> = (model.schema().uninterpreted().iter())
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        keys,
        [
            "shape S: members.D.newKey",
            "shape S: members.D.document",
            "shape S: members.U.document",
            "shape S: members.U.union",
        ]
    );
    let s = named(model.schema(), "S")?;
    assert!(cold(member(s, "D")?, traits::DOCUMENT));
    assert!(!cold(member(s, "U")?, traits::DOCUMENT));
    Ok(())
}

/// A falsy key on a reference clears its target's flag, and a `location`
/// on one combines with its target's `locationName`, before either is
/// read: the merge is key by key.
#[test]
fn a_reference_s_keys_overlay_its_target_s_key_by_key() -> Result<()> {
    let flags = crate::schema::projection::FLAG_NAMES;
    let flags: serde_json::Map<String, Json> = flags
        .iter()
        .map(|key| ((*key).to_owned(), json!(true)))
        .collect();
    let mut cleared: serde_json::Map<String, Json> = flags
        .keys()
        .map(|key| (key.clone(), json!(false)))
        .collect();
    cleared.insert("shape".into(), json!("Flagged"));
    let mut named = flags.clone();
    named.insert("type".into(), json!("structure"));
    named.insert("members".into(), json!({}));
    named.insert("locationName".into(), json!("wire"));
    let model = json!({"shapes": {
        "Flagged": named,
        "S": {"type": "structure", "members": {
            "Cleared": cleared,
            "Header": {"shape": "Flagged", "location": "header"}}}}});
    let projection = Model::from_value(&model)?.projection();
    let line = |label: &str| {
        let found = projection.lines().find(|line| line.contains(label));
        found.unwrap_or_default().to_owned()
    };
    assert!(line("shape \"Flagged\"").contains("flags="), "{projection}");
    assert!(
        !line("member \"Cleared\"").contains("flags="),
        "{projection}"
    );
    assert!(line("member \"Header\"").contains(r#"location="header" locationName="wire""#));
    Ok(())
}

/// Every fake model under `tests/models` loads with its overlay.
#[test]
fn every_fake_model_loads() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models");
    let source = DirSource::new(root);
    let services = source.services(Kind::Service)?;
    for service in &services {
        let model = Model::load(&source, service).map_err(|e| format!("{service}: {e}"))?;
        assert!(model.metadata().protocol.is_some(), "{service}");
    }
    assert!(services.len() >= 9, "{}", services.len());
    Ok(())
}

/// Each protocol-suite case's description: the suite's, with the case as
/// its only operation, as the suite harness builds it.
fn suite_models() -> Result<Vec<(String, Json)>> {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../aws-cli/tests/unit/botocore/protocols");
    let mut models = Vec::new();
    for direction in ["input", "output"] {
        let mut files: Vec<_> = std::fs::read_dir(root.join(direction))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::result::Result<_, _>>()?;
        files.sort();
        for file in files {
            let suite: Json = serde_json::from_slice(&std::fs::read(&file)?)?;
            let name = file.file_name().unwrap_or_default().to_string_lossy();
            for (g, group) in suite.as_array().into_iter().flatten().enumerate() {
                for (c, case) in group["cases"].as_array().into_iter().flatten().enumerate() {
                    let mut model = group.clone();
                    let object = model.as_object_mut().ok_or("group")?;
                    object.remove("cases");
                    let operation = case["given"]["name"].as_str().unwrap_or("OperationName");
                    object.insert(
                        "operations".into(),
                        json!({ operation: case["given"].clone() }),
                    );
                    models.push((format!("{direction}/{name} {g}.{c}"), model));
                }
            }
        }
    }
    Ok(models)
}

/// Every protocol-suite description builds, with its one operation.
#[test]
fn every_protocol_suite_model_builds() -> Result<()> {
    let models = suite_models()?;
    for (label, json) in &models {
        let model = Model::from_value(json).map_err(|e| format!("{label}: {e}"))?;
        assert_eq!(model.schema().service().operations().len(), 1, "{label}");
    }
    assert!(models.len() >= 700, "{}", models.len());
    Ok(())
}

/// Each value of `json` replaced by each wrong-typed stand-in, and each
/// object key removed, one at a time.
fn mutants(json: &Json) -> Vec<Json> {
    fn walk(at: &Json, root: &Json, path: &mut Vec<String>, out: &mut Vec<Json>) {
        let stand_ins = [Json::Null, json!("x"), json!({}), json!([1]), json!(1.5)];
        for stand_in in stand_ins {
            let mut mutant = root.clone();
            if let Some(slot) = pointer(&mut mutant, path) {
                *slot = stand_in;
                out.push(mutant);
            }
        }
        for (key, child) in at.as_object().into_iter().flatten() {
            let mut mutant = root.clone();
            if let Some(Json::Object(parent)) = pointer(&mut mutant, path) {
                parent.remove(key);
                out.push(mutant);
            }
            path.push(key.clone());
            walk(child, root, path, out);
            path.pop();
        }
    }
    fn pointer<'a>(json: &'a mut Json, path: &[String]) -> Option<&'a mut Json> {
        path.iter().try_fold(json, |at, key| at.get_mut(key))
    }
    let mut out = Vec::new();
    walk(json, json, &mut Vec::new(), &mut out);
    out
}

/// The first case of every suite group, mutated: wrong-typed values and
/// missing keys at every level build a model that renders, or are refused
/// with an error that names something.
#[test]
fn mutated_models_build_or_are_refused() -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    let mut count = 0;
    for (label, model) in suite_models()? {
        let group = label.rsplit_once('.').map_or(&*label, |(group, _)| group);
        if !seen.insert(group.to_owned()) {
            continue;
        }
        for mutant in mutants(&model) {
            count += 1;
            match Model::from_value(&mutant) {
                Ok(model) => assert!(model.projection().starts_with("service "), "{label}"),
                Err(error) => assert!(!error.to_string().is_empty(), "{label}"),
            }
        }
    }
    assert!(count >= 10_000, "only {count} mutants");
    Ok(())
}

/// The fuzz corpus of `model_json`, through the fuzz body.
#[test]
fn the_model_json_corpus_reads_without_a_panic() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/model_json");
    let mut count = 0;
    for entry in std::fs::read_dir(root)? {
        crate::fuzz::model_json(&std::fs::read(entry?.path())?);
        count += 1;
    }
    assert!(count > 0);
    Ok(())
}

/// Prints parse and build time per service, best of seven. Run in release
/// mode: `RUST_TEST_NOCAPTURE=1 cargo xtask test -p model --profile
/// release-safe --filter --ignored`.
#[test]
#[ignore = "a timing report, not a check"]
fn build_timings() -> Result<()> {
    let source = DirSource::new(testing::data_dir());
    let mut report =
        String::from("c2j-build service  bytes  parse_ms  build_ms  operations/shapes\n");
    for service in ["sts", "dynamodb", "s3", "ec2"] {
        let version = crate::source::latest_version(&source, service, Kind::Service)?;
        let bytes = source
            .read(&format!("{service}/{version}/service-2"))?
            .ok_or(service)?;
        let (mut parse, mut build) = (std::time::Duration::MAX, std::time::Duration::MAX);
        let mut counts = (0, 0);
        for _ in 0..7 {
            let started = std::time::Instant::now();
            let json: Json = serde_json::from_slice(&bytes)?;
            let parsed = started.elapsed();
            let set = super::build(&json, &super::namespace(service))?;
            build = build.min(started.elapsed() - parsed);
            parse = parse.min(parsed);
            counts = (set.service().operations().len(), set.named().len());
        }
        report.push_str(&format!(
            "c2j-build {service:<9} {:>8} {:>9.2} {:>9.2}  {}/{}\n",
            bytes.len(),
            parse.as_secs_f64() * 1e3,
            build.as_secs_f64() * 1e3,
            counts.0,
            counts.1
        ));
    }
    std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes())?;
    Ok(())
}
