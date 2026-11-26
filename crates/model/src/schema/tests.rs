use super::*;
use crate::testing::Result;
use crate::value::Value;
use serde_json::json;

const INHERIT: MemberTraits = MemberTraits::Inherit;

fn error_traits(builder: &mut SchemaSetBuilder, code: &str) -> Result<KnownTraits> {
    let mut known = KnownTraits::default();
    known.flags.set(Flag::Error, true);
    known.error = Some(ErrorInfo {
        code: Some(builder.intern(code)?),
        http_status: Some(400),
        sender_fault: true,
    });
    Ok(known)
}

/// The shapes of [`fixture`]: a string, a structure whose members are not
/// in name order, a list, a map, an output referenced before it is
/// declared, and error shapes.
fn shapes(builder: &mut SchemaSetBuilder) -> Result<[DraftId; 2]> {
    let output = builder.reference("Output")?;
    let string = builder.declare("String", ShapeType::String, None)?;
    let text = KnownTraits {
        pattern: Some(builder.intern("^a")?),
        min: Some(Bound::Integer(1)),
        ..KnownTraits::default()
    };
    builder.set_traits(string, text.clone(), ColdTraits::default())?;
    let input = builder.declare("Input", ShapeType::Structure, None)?;
    builder.add_member(input, "Zeta", string, INHERIT)?;
    let mut alpha = text.clone();
    alpha.json_name = Some(builder.intern("alpha")?);
    alpha.location = Some(builder.intern("header")?);
    alpha.location_name = Some(builder.intern("X-Alpha")?);
    builder.add_member(
        input,
        "Alpha",
        string,
        MemberTraits::resolved(alpha, ColdTraits::default()),
    )?;
    builder.add_member(
        input,
        "Same",
        string,
        MemberTraits::resolved(text, ColdTraits::default()),
    )?;
    let integer = builder.prelude("Integer").ok_or("no Integer")?;
    builder.add_member(input, "Count", integer, INHERIT)?;
    let required = KnownTraits {
        required: Some(vec![builder.intern("Alpha")?, builder.intern("Zeta")?].into()),
        ..KnownTraits::default()
    };
    builder.set_traits(input, required, ColdTraits::default())?;
    let tags = builder.declare("Tags", ShapeType::List, None)?;
    builder.set_list_member(tags, string, INHERIT)?;
    builder.add_member(input, "Tags", tags, INHERIT)?;
    let attrs = builder.declare("Attrs", ShapeType::Map, None)?;
    builder.set_map_entry(attrs, (string, INHERIT), (tags, INHERIT))?;
    builder.declare("Output", ShapeType::Structure, None)?;
    builder.add_member(output, "Attrs", attrs, INHERIT)?;
    let mut document = ColdTraits::default();
    document.insert("c2j#document", json!(true))?;
    builder.set_traits(output, KnownTraits::default(), document)?;
    Ok([input, output])
}

fn fixture() -> Result<SchemaSet> {
    let mut builder = SchemaSetBuilder::new("c2j.svc")?;
    let [input, output] = shapes(&mut builder)?;
    let mut errors = Vec::new();
    for (name, code) in [
        ("NotFound", ""),
        ("Throttled", "Throttling"),
        ("Again", "Throttling"),
    ] {
        let shape = builder.declare(name, ShapeType::Structure, None)?;
        let known = error_traits(&mut builder, code)?;
        builder.set_traits(shape, known, ColdTraits::default())?;
        errors.push(shape);
    }
    let plain = builder.declare("Plain", ShapeType::Structure, None)?;
    let mut known = error_traits(&mut builder, "Plain")?;
    known.flags.set(Flag::Error, false);
    builder.set_traits(plain, known, ColdTraits::default())?;
    let root = KnownTraits {
        xml_name: Some(builder.intern("InputRoot")?),
        ..KnownTraits::default()
    };
    let input = builder.root_overlay(input, MemberTraits::resolved(root, ColdTraits::default()))?;
    let output = builder.root_overlay(output, INHERIT)?;
    let shapes = OperationShapes {
        input: Some(input),
        output: Some(output),
        errors: errors[..2].to_vec(),
    };
    builder.add_operation(get_thing(), shapes)?;
    let unit = builder.prelude("Unit");
    let ping = OperationData {
        name: "Ping".into(),
        ..OperationData::default()
    };
    let shapes = OperationShapes {
        input: unit,
        output: unit,
        errors: Vec::new(),
    };
    builder.add_operation(ping, shapes)?;
    builder.set_service(service());
    builder.record_uninterpreted(Site::Service, "metadata.newKey");
    builder.record_uninterpreted(Site::Operation("Ping".into()), "http.timeout");
    builder.record_uninterpreted(Site::Shape("Attrs".into()), "newTrait");
    builder.record_uninterpreted(Site::Shape("Plain".into()), "other");
    Ok(builder.finish()?)
}

fn get_thing() -> OperationData {
    OperationData {
        name: "GetThing".into(),
        wire_name: Some("GetThingWire".into()),
        http_method: Some("POST".into()),
        http_request_uri: Some("/{Id}".into()),
        auth: Some(vec!["aws.auth#sigv4".into()].into()),
        auth_type: Some("v4".into()),
        endpoint_discovery: Some(EndpointDiscovery { required: true }),
        static_context_params: vec![("Flag".into(), Value::Bool(true))].into(),
        operation_context_params: vec![("Ids".into(), "Things[*].Id".into())].into(),
        ..OperationData::default()
    }
}

fn service() -> ServiceData {
    ServiceData {
        version: Some("2020-01-01".into()),
        sdk_id: Some("Svc".into()),
        sigv4_name: Some("svc".into()),
        query_compatible: true,
        protocol_traits: vec![traits::AWS_JSON_1_1].into(),
        ..ServiceData::default()
    }
}

fn named<'s>(set: &'s SchemaSet, name: &str) -> Result<Schema<'s>> {
    Ok(set
        .shape_named(name)
        .ok_or_else(|| format!("no shape {name}"))?)
}

fn member<'s>(set: &'s SchemaSet, shape: &str, name: &str) -> Result<Schema<'s>> {
    let found = named(set, shape)?.member(name);
    Ok(found.ok_or_else(|| format!("no member {shape}${name}"))?)
}

fn names<'s>(schemas: impl Iterator<Item = Schema<'s>>) -> Vec<&'s str> {
    schemas
        .map(|schema| schema.member_name().unwrap_or(schema.id().name()))
        .collect()
}

#[test]
fn named_shapes_keep_document_order() -> Result<()> {
    let set = fixture()?;
    let expected = [
        "String",
        "Input",
        "Tags",
        "Attrs",
        "Output",
        "NotFound",
        "Throttled",
    ];
    assert_eq!(names(set.named())[..7], expected);
    assert_eq!(set.named().len(), 9);
    assert_eq!(named(&set, "Output")?.id().as_str(), "c2j.svc#Output");
    assert_eq!(set.shape_named("Integer"), None);
    let ours = named(&set, "String")?;
    let prelude = set.prelude("String").ok_or("no prelude String")?;
    assert_ne!(ours, prelude);
    assert_eq!(prelude.id().as_str(), "smithy.api#String");
    assert!(prelude.is_prelude() && !ours.is_prelude());
    Ok(())
}

#[test]
fn members_keep_declaration_order() -> Result<()> {
    let set = fixture()?;
    let input = named(&set, "Input")?;
    assert_eq!(
        names(input.members()),
        ["Zeta", "Alpha", "Same", "Count", "Tags"]
    );
    assert_eq!(input.members().len(), 5);
    let indices: Vec<_> = input.members().map(Schema::member_index).collect();
    assert_eq!(indices, [0, 1, 2, 3, 4].map(Some));
    assert_eq!(
        member(&set, "Input", "Alpha")?.id().as_str(),
        "c2j.svc#Input$Alpha"
    );
    assert_eq!(input.member("alpha"), None);
    assert_eq!(input.required().collect::<Vec<_>>(), ["Alpha", "Zeta"]);
    Ok(())
}

#[test]
fn members_report_their_target() -> Result<()> {
    let set = fixture()?;
    let count = member(&set, "Input", "Count")?;
    assert!(count.is_member());
    assert_eq!(
        (count.shape_type(), count.type_name()),
        (ShapeType::Integer, "integer")
    );
    assert_eq!(count.target(), set.prelude("Integer").ok_or("no Integer")?);
    let zeta = member(&set, "Input", "Zeta")?;
    assert_eq!(zeta.target(), named(&set, "String")?);
    assert_eq!(zeta.target().target(), zeta.target());
    assert_eq!(zeta.target().member_name(), None);
    Ok(())
}

#[test]
fn references_that_add_nothing_share_the_target_traits() -> Result<()> {
    let set = fixture()?;
    let string = named(&set, "String")?;
    let zeta = member(&set, "Input", "Zeta")?;
    let same = member(&set, "Input", "Same")?;
    assert_eq!(
        (zeta.has_own_traits(), same.has_own_traits()),
        (false, false)
    );
    assert!(std::ptr::eq(zeta.known(), string.known()));
    assert!(std::ptr::eq(same.known(), string.known()));
    assert_eq!(
        (zeta.pattern(), zeta.http_binding()),
        (Some("^a"), HttpBinding::Body)
    );
    Ok(())
}

#[test]
fn member_traits_are_the_target_overlaid() -> Result<()> {
    let set = fixture()?;
    let alpha = member(&set, "Input", "Alpha")?;
    assert!(alpha.has_own_traits());
    assert_eq!(
        (alpha.json_name(), alpha.pattern()),
        (Some("alpha"), Some("^a"))
    );
    assert_eq!(alpha.http_binding(), HttpBinding::Header(Some("X-Alpha")));
    Ok(())
}

#[test]
fn has_trait_reads_the_slot_or_the_cold_map() -> Result<()> {
    let set = fixture()?;
    let alpha = member(&set, "Input", "Alpha")?;
    let zeta = member(&set, "Input", "Zeta")?;
    let ids = [
        traits::JSON_NAME,
        traits::LENGTH,
        traits::RANGE,
        traits::HTTP,
    ];
    let has = |schema: Schema<'_>| ids.map(|id| schema.has_trait(id));
    assert_eq!(has(alpha), [true, true, false, false]);
    assert_eq!(has(zeta), [false, true, false, false]);
    Ok(())
}

#[test]
fn lists_and_maps_have_their_members() -> Result<()> {
    let set = fixture()?;
    let tags = named(&set, "Tags")?;
    let member = tags.list_member().ok_or("no list member")?;
    assert_eq!(member.id().as_str(), "c2j.svc#Tags$member");
    assert_eq!(member.target(), named(&set, "String")?);
    assert_eq!(tags.members().len(), 0);
    let attrs = named(&set, "Attrs")?;
    let key = attrs.map_key().ok_or("no key")?;
    let value = attrs.map_value().ok_or("no value")?;
    assert_eq!(
        (key.id().as_str(), key.member_index()),
        ("c2j.svc#Attrs$key", Some(0))
    );
    assert_eq!(
        (value.id().member(), value.member_index()),
        (Some("value"), Some(1))
    );
    assert_eq!(
        value.list_member().map(Schema::target),
        Some(named(&set, "String")?)
    );
    assert_eq!(named(&set, "Input")?.list_member(), None);
    assert_eq!(tags.map_key(), None);
    Ok(())
}

#[test]
fn many_members_are_found_by_name() -> Result<()> {
    let mut builder = SchemaSetBuilder::new("t")?;
    let big = builder.declare("Big", ShapeType::Structure, None)?;
    let string = builder.prelude("String").ok_or("no String")?;
    for index in (0..20).rev() {
        builder.add_member(big, &format!("M{index:02}"), string, INHERIT)?;
    }
    let set = builder.finish()?;
    let big = named(&set, "Big")?;
    assert_eq!(names(big.members())[..2], ["M19", "M18"]);
    for index in 0..20 {
        let found = big.member(&format!("M{index:02}")).ok_or("missing")?;
        assert_eq!(found.member_index(), Some(19 - index));
    }
    assert_eq!(big.member("M20"), None);
    Ok(())
}

#[test]
fn the_prelude_is_outside_the_named_range() -> Result<()> {
    let set = fixture()?;
    assert_eq!(set.len(), set.named().len() + 5 + 4 + 1 + PRELUDE.len());
    for (name, shape_type) in PRELUDE {
        let schema = set.prelude(name).ok_or("missing")?;
        assert_eq!(
            (schema.id().namespace(), schema.shape_type()),
            ("smithy.api", shape_type)
        );
        assert!(set.named().all(|named| named != schema));
    }
    let ping = set.service().operation("Ping").ok_or("no Ping")?;
    assert_eq!((ping.input(), ping.output()), (None, None));
    Ok(())
}

#[test]
fn root_overlays_share_the_target_members() -> Result<()> {
    let set = fixture()?;
    let operation = set.service().operation("GetThing").ok_or("no GetThing")?;
    let input = operation.input().ok_or("no input")?;
    let target = named(&set, "Input")?;
    assert_ne!(input, target);
    assert_eq!(input.id(), target.id());
    assert!(input.has_own_traits() && !input.is_member());
    assert_eq!(
        (input.xml_name(), target.xml_name()),
        (Some("InputRoot"), None)
    );
    assert!(input.members().eq(target.members()));
    assert_eq!(input.target(), input);
    assert!(set.named().all(|named| named != input));
    assert_eq!(operation.output(), Some(named(&set, "Output")?));
    Ok(())
}

#[test]
fn the_registry_reads_error_shapes() -> Result<()> {
    let set = fixture()?;
    let registry = set.registry();
    assert_eq!(
        names(registry.error_shapes()),
        ["NotFound", "Throttled", "Again"]
    );
    assert_eq!(
        registry.error_code(named(&set, "NotFound")?),
        Some("NotFound")
    );
    assert_eq!(
        registry.error_code(named(&set, "Throttled")?),
        Some("Throttling")
    );
    assert_eq!(registry.error_code(named(&set, "Plain")?), None);
    let entry = |code| registry.entry_for_error_code(code);
    assert_eq!(entry("Throttling"), Some(named(&set, "Again")?));
    assert_eq!(entry("NotFound"), Some(named(&set, "NotFound")?));
    assert_eq!(entry("Plain"), None);
    let error = named(&set, "Again")?.error().ok_or("no error")?;
    assert_eq!((error.http_status, error.sender_fault), (Some(400), true));
    Ok(())
}

#[test]
fn operations_read_their_data() -> Result<()> {
    let set = fixture()?;
    let service = set.service();
    let operations: Vec<_> = service.operations().map(OperationSchema::name).collect();
    assert_eq!(operations, ["GetThing", "Ping"]);
    assert_eq!(
        service.operation("Missing").map(OperationSchema::name),
        None
    );
    let op = service.operation("GetThing").ok_or("no GetThing")?;
    assert_eq!(set.operation(op.index()).name(), "GetThing");
    let texts = [
        Some(op.id().as_str()),
        Some(op.id().name()),
        op.wire_name(),
        op.http_method(),
        op.http_request_uri(),
        op.auth_type(),
        op.host_prefix(),
    ];
    let expected = [
        "c2j.svc#GetThing",
        "GetThing",
        "GetThingWire",
        "POST",
        "/{Id}",
        "v4",
    ];
    assert_eq!(
        texts,
        expected
            .map(Some)
            .into_iter()
            .chain([None])
            .collect::<Vec<_>>()[..]
    );
    let auth = op.auth().map(Iterator::collect::<Vec<_>>);
    assert_eq!(auth, Some(vec!["aws.auth#sigv4"]));
    assert_eq!(
        op.endpoint_discovery(),
        Some(EndpointDiscovery { required: true })
    );
    Ok(())
}

#[test]
fn operations_read_their_shapes_parameters_and_flags() -> Result<()> {
    let set = fixture()?;
    let op = set.service().operation("GetThing").ok_or("no GetThing")?;
    assert_eq!(names(op.errors()), ["NotFound", "Throttled"]);
    let statics: Vec<_> = op.static_context_params().collect();
    assert_eq!(statics, [("Flag", &Value::Bool(true))]);
    let paths: Vec<_> = op.operation_context_params().collect();
    assert_eq!(paths, [("Ids", "Things[*].Id")]);
    let flags = [
        op.unsigned_payload(),
        op.http_checksum_required(),
        op.endpoint_operation(),
    ];
    assert_eq!(flags, [false; 3]);
    assert!(op.request_compression().is_none() && op.http_checksum().is_none());
    Ok(())
}

#[test]
fn the_service_reads_its_data() -> Result<()> {
    let set = fixture()?;
    let service = set.service();
    let texts = [
        service.version(),
        service.sdk_id(),
        service.sigv4_name(),
        service.endpoint_prefix(),
        service.xml_namespace(),
    ];
    assert_eq!(
        texts,
        [Some("2020-01-01"), Some("Svc"), Some("svc"), None, None]
    );
    assert!(service.query_compatible() && service.auth().is_none());
    assert_eq!(service.protocol_traits(), [traits::AWS_JSON_1_1]);
    assert_eq!(service.client_context_params(), []);
    Ok(())
}

#[test]
fn a_call_reaches_the_uninterpreted_keys_of_its_shapes() -> Result<()> {
    let set = fixture()?;
    assert_eq!(set.uninterpreted().len(), 4);
    let reached = |name| -> Result<Vec<String>> {
        let operation = set.service().operation(name).ok_or("no operation")?;
        let keys = set.uninterpreted_reached(operation);
        Ok(keys.iter().map(ToString::to_string).collect())
    };
    assert_eq!(
        reached("GetThing")?,
        ["metadata.newKey", "shape Attrs: newTrait"]
    );
    assert_eq!(
        reached("Ping")?,
        ["metadata.newKey", "operation Ping: http.timeout"]
    );
    Ok(())
}

#[test]
fn cold_traits_are_kept_by_id() -> Result<()> {
    let set = fixture()?;
    let output = named(&set, "Output")?;
    assert_eq!(output.cold_trait("c2j#document"), Some(&json!(true)));
    assert!(output.has_trait(traits::DOCUMENT) && !output.has_trait(traits::EVENT));
    assert_eq!(
        output.cold_traits().collect::<Vec<_>>(),
        [("c2j#document", &json!(true))]
    );
    assert_eq!(named(&set, "Input")?.cold_traits().len(), 0);
    Ok(())
}

#[test]
fn sets_compare_and_clone() -> Result<()> {
    let set = fixture()?;
    assert_eq!(set, fixture()?);
    assert_eq!(set.clone(), set);
    assert!(!set.is_empty());
    Ok(())
}
