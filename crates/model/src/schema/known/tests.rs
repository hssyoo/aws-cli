use super::*;
use crate::schema::{ColdTraits, SchemaSetBuilder, ShapeId, Site};
use crate::testing::Result;
use serde_json::json;
use std::collections::HashSet;

/// The slot ids an operation or the service holds, not a schema.
const NON_SHAPE_SLOTS: [TraitId; 23] = [
    traits::HTTP,
    traits::ENDPOINT,
    traits::AUTH,
    traits::AUTH_TYPE,
    traits::UNSIGNED_PAYLOAD,
    traits::HTTP_CHECKSUM_REQUIRED,
    traits::HTTP_CHECKSUM,
    traits::REQUEST_COMPRESSION,
    traits::CLIENT_DISCOVERED_ENDPOINT,
    traits::ENDPOINT_OPERATION,
    traits::STATIC_CONTEXT_PARAMS,
    traits::OPERATION_CONTEXT_PARAMS,
    traits::AWS_SERVICE,
    traits::SIGV4,
    traits::AWS_QUERY_COMPATIBLE,
    traits::CLIENT_CONTEXT_PARAMS,
    traits::AWS_JSON_1_0,
    traits::AWS_JSON_1_1,
    traits::REST_JSON_1,
    traits::REST_XML,
    traits::AWS_QUERY,
    traits::EC2_QUERY,
    traits::RPC_V2_CBOR,
];

#[test]
fn known_ids_are_unique_shape_ids_and_round_trip() -> Result<()> {
    let mut seen = HashSet::new();
    for (index, (text, storage)) in KNOWN.iter().enumerate() {
        assert!(seen.insert(*text), "{text} twice");
        assert_eq!(ShapeId::parse(text)?.member(), None, "{text}");
        let id = TraitId::parse(text).ok_or("no id")?;
        assert_eq!(
            (id.as_str(), id.storage(), usize::from(id.0)),
            (*text, *storage, index)
        );
    }
    assert_eq!(traits::JSON_NAME.as_str(), "smithy.api#jsonName");
    assert_eq!(traits::RESULT_WRAPPER.as_str(), "c2j#resultWrapper");
    assert_eq!(traits::DOCUMENTATION.storage(), Storage::Ignored);
    assert_eq!(traits::EVENT.storage(), Storage::Cold);
    assert_eq!(TraitId::parse("smithy.api#noSuchTrait"), None);
    Ok(())
}

/// Every slot id has a schema slot or is an operation's or the service's,
/// never both; every flag bit is distinct and its id a slot.
#[test]
fn every_slot_id_has_one_home() {
    let known = KnownTraits::default();
    for (index, (text, storage)) in KNOWN.iter().enumerate() {
        let id = TraitId(u16::try_from(index).unwrap_or(u16::MAX));
        let shape_slot = known.has_slot(id, ShapeType::String).is_some();
        let elsewhere = NON_SHAPE_SLOTS.contains(&id);
        let expected = *storage == Storage::Slot;
        assert_eq!(shape_slot || elsewhere, expected, "{text}");
        assert!(!(shape_slot && elsewhere), "{text}");
    }
    let bits: HashSet<u32> = FLAGS.iter().map(|(flag, _)| *flag as u32).collect();
    assert_eq!(bits.len(), FLAGS.len());
    assert!(bits.iter().all(|bit| bit.count_ones() == 1));
    assert!(FLAGS.iter().all(|(_, id)| id.storage() == Storage::Slot));
}

#[test]
fn flags_set_and_clear() {
    let mut flags = TraitFlags::default();
    flags.set(Flag::XmlFlattened, true);
    flags.set(Flag::Sensitive, true);
    flags.set(Flag::XmlFlattened, false);
    assert!(!flags.is(Flag::XmlFlattened));
    assert!(flags.is(Flag::Sensitive));
}

#[test]
fn bounds_print_as_written() {
    assert_eq!(Bound::Integer(10).to_string(), "10");
    assert_eq!(Bound::Float(1.0).to_string(), "1.0");
    assert_eq!(Bound::Float(1e20).to_string(), "1e+20");
}

#[test]
fn http_bindings_apply_no_defaults() {
    let name = Some("X-Name");
    let cases = [
        (None, HttpBinding::Body),
        (Some("header"), HttpBinding::Header(name)),
        (Some("headers"), HttpBinding::PrefixHeaders(name)),
        (Some("querystring"), HttpBinding::Query(name)),
        (Some("uri"), HttpBinding::Label(name)),
        (Some("statusCode"), HttpBinding::ResponseCode(name)),
        (Some("Header"), HttpBinding::Other("Header")),
    ];
    for (location, binding) in cases {
        assert_eq!(HttpBinding::of(location, name), binding, "{location:?}");
    }
    assert_eq!(
        HttpBinding::of(Some("header"), None),
        HttpBinding::Header(None)
    );
}

#[test]
fn timestamp_formats_match_as_botocore_lower_cases() {
    assert_eq!(TimestampFormat::of("iso8601"), TimestampFormat::Iso8601);
    assert_eq!(TimestampFormat::of("ISO8601"), TimestampFormat::Iso8601);
    assert_eq!(
        TimestampFormat::of("unixTimestamp"),
        TimestampFormat::UnixTimestamp
    );
    assert_eq!(TimestampFormat::of("RFC822"), TimestampFormat::Rfc822);
    assert_eq!(
        TimestampFormat::of("iso 8601"),
        TimestampFormat::Other("iso 8601")
    );
}

#[test]
fn smithy_merge_lets_the_member_win_and_flags_add() -> Result<()> {
    let mut builder = SchemaSetBuilder::new("t")?;
    let [a, b, c] = ["a", "b", "c"].map(|text| builder.intern(text));
    let (a, b, c) = (a?, b?, c?);
    let mut target = KnownTraits::default();
    target.flags.set(Flag::Sensitive, true);
    target.json_name = Some(a);
    target.pattern = Some(b);
    let mut member = KnownTraits::default();
    member.flags.set(Flag::HostLabel, true);
    member.json_name = Some(c);
    member.min = Some(Bound::Integer(1));
    let merged = target.overlaid(&member);
    assert!(merged.flags.is(Flag::Sensitive) && merged.flags.is(Flag::HostLabel));
    assert_eq!(merged.json_name, Some(c));
    assert_eq!(merged.pattern, Some(b));
    assert_eq!(merged.min, Some(Bound::Integer(1)));
    assert_eq!(KnownTraits::default().overlaid(&target), target);
    Ok(())
}

#[test]
fn cold_maps_take_only_what_belongs_there() -> Result<()> {
    let mut cold = ColdTraits::default();
    cold.insert("c2j#event", json!(true))?;
    cold.insert("smithy.api#documentation", json!("text"))?;
    assert_eq!(cold.get("c2j#event"), Some(&json!(true)));
    let refusals = [
        cold.insert("smithy.api#jsonName", json!("x")),
        cold.insert("acme#custom", json!(1)),
        cold.insert("c2j#event", json!(false)),
        cold.insert_unknown("smithy.api#jsonName", json!(1), Site::Service, "k"),
        cold.insert_unknown("acme#a$b", json!(1), Site::Service, "k"),
        cold.insert_unknown("c2j#newKey", json!(1), Site::Service, "k"),
        cold.insert_unknown("acme", json!(1), Site::Service, "k"),
    ];
    let kinds: Vec<_> = refusals.iter().map(|r| format!("{r:?}")).collect();
    let expected = [
        "SlotTrait",
        "UnknownTrait",
        "DuplicateTrait",
        "KnownTrait",
        "UnknownTrait",
        "UnknownTrait",
        "ShapeId",
    ];
    for (kind, expected) in kinds.iter().zip(expected) {
        assert!(
            kind.starts_with(&format!("Err({expected}(")),
            "{kind} for {expected}"
        );
    }
    cold.insert_unknown("acme#custom", json!(1), Site::Service, "acme#custom")?;
    assert_eq!(cold.get("acme#custom"), Some(&json!(1)));
    Ok(())
}

#[test]
fn cold_merge_replaces_and_appends() -> Result<()> {
    let mut target = ColdTraits::default();
    target.insert("c2j#event", json!(1))?;
    target.insert("c2j#union", json!(2))?;
    let mut member = ColdTraits::default();
    member.insert("c2j#union", json!(3))?;
    member.insert("c2j#document", json!(4))?;
    let merged = target.overlaid(&member);
    let mut expected = ColdTraits::default();
    expected.insert("c2j#event", json!(1))?;
    expected.insert("c2j#union", json!(3))?;
    expected.insert("c2j#document", json!(4))?;
    assert_eq!(merged, expected);
    Ok(())
}
