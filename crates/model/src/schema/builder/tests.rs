//! The builder's refusals: each test is one way a front-end could hand the
//! builder a set that reads wrong, and the refusal that stops it.
use super::*;
use crate::schema::Flag;
use crate::shapes::ModelError;
use crate::testing::Result;
use serde_json::json;

const INHERIT: MemberTraits = MemberTraits::Inherit;

fn builder() -> Result<SchemaSetBuilder> {
    Ok(SchemaSetBuilder::new("t")?)
}

fn string(builder: &SchemaSetBuilder) -> Result<DraftId> {
    Ok(builder.prelude("String").ok_or("no String")?)
}

/// The error's variant name, as the tests compare refusals.
fn kind<T: std::fmt::Debug>(result: std::result::Result<T, BuildError>) -> String {
    match result {
        Ok(value) => format!("Ok({value:?})"),
        Err(error) => format!("{error:?}")
            .split('(')
            .next()
            .unwrap_or("")
            .to_owned(),
    }
}

#[test]
fn a_shape_declared_twice_is_refused() -> Result<()> {
    let mut b = builder()?;
    b.declare("A", ShapeType::String, None)?;
    assert_eq!(kind(b.declare("A", ShapeType::Integer, None)), "Duplicate");
    assert_eq!(
        kind(b.declare_id("smithy.api#String", ShapeType::String, None)),
        "Duplicate"
    );
    Ok(())
}

#[test]
fn a_reference_never_declared_is_refused_at_finish() -> Result<()> {
    let mut b = builder()?;
    let s = b.declare("S", ShapeType::Structure, None)?;
    let missing = b.reference("Missing")?;
    b.add_member(s, "M", missing, INHERIT)?;
    assert_eq!(
        b.finish().err(),
        Some(BuildError::NoShapeFound("t#Missing".into()))
    );
    let mut b = builder()?;
    let later = b.reference("Later")?;
    let s = b.declare("S", ShapeType::Structure, None)?;
    b.add_member(s, "M", later, INHERIT)?;
    assert_eq!(b.declare("Later", ShapeType::Long, None)?, later);
    let set = b.finish()?;
    let member = set
        .shape_named("S")
        .and_then(|s| s.member("M"))
        .ok_or("no member")?;
    assert_eq!(member.shape_type(), ShapeType::Long);
    Ok(())
}

#[test]
fn two_named_shapes_with_one_short_name_clash() -> Result<()> {
    let mut b = builder()?;
    b.declare_id("a#Same", ShapeType::String, None)?;
    b.declare_id("b#Same", ShapeType::String, None)?;
    assert_eq!(b.finish().err(), Some(BuildError::NameClash("Same".into())));
    Ok(())
}

#[test]
fn malformed_names_are_refused() -> Result<()> {
    let mut b = builder()?;
    let s = b.declare("S", ShapeType::Structure, None)?;
    let string = string(&b)?;
    assert_eq!(kind(b.declare("A#B", ShapeType::String, None)), "ShapeId");
    assert_eq!(kind(b.declare("", ShapeType::String, None)), "ShapeId");
    assert_eq!(
        kind(b.declare_id("t#S$m", ShapeType::String, None)),
        "InvalidShape"
    );
    assert_eq!(kind(b.reference_id("t#S$m")), "InvalidShape");
    assert_eq!(kind(b.add_member(s, "a$b", string, INHERIT)), "ShapeId");
    assert_eq!(kind(b.add_member(s, "", string, INHERIT)), "ShapeId");
    let named = OperationData {
        name: "Op#1".into(),
        ..OperationData::default()
    };
    assert_eq!(
        kind(b.add_operation(named, OperationShapes::default())),
        "ShapeId"
    );
    Ok(())
}

#[test]
fn an_unknown_type_needs_its_text() -> Result<()> {
    let mut b = builder()?;
    assert_eq!(
        kind(b.declare("U", ShapeType::Unknown, None)),
        "InvalidShape"
    );
    b.declare("U", ShapeType::Unknown, Some("decimal"))?;
    b.declare("D", ShapeType::Document, None)?;
    let set = b.finish()?;
    let type_name = |name| set.shape_named(name).map(|s| s.type_name());
    assert_eq!(
        (type_name("U"), type_name("D")),
        (Some("decimal"), Some("structure"))
    );
    Ok(())
}

#[test]
fn members_go_only_where_the_type_holds_them() -> Result<()> {
    let mut b = builder()?;
    let string = string(&b)?;
    let text = b.declare("Text", ShapeType::String, None)?;
    let list = b.declare("List", ShapeType::List, None)?;
    let map = b.declare("Map", ShapeType::Map, None)?;
    let s = b.declare("S", ShapeType::Structure, None)?;
    let placeholder = b.reference("Later")?;
    assert_eq!(
        kind(b.add_member(text, "M", string, INHERIT)),
        "InvalidParent"
    );
    assert_eq!(
        kind(b.add_member(list, "M", string, INHERIT)),
        "InvalidParent"
    );
    assert_eq!(
        kind(b.add_member(placeholder, "M", string, INHERIT)),
        "InvalidParent"
    );
    assert_eq!(
        kind(b.add_member(string, "M", string, INHERIT)),
        "InvalidParent"
    );
    assert_eq!(kind(b.set_list_member(s, string, INHERIT)), "InvalidParent");
    let entry = || ((string, INHERIT), (string, INHERIT));
    let (key, value) = entry();
    assert_eq!(kind(b.set_map_entry(list, key, value)), "InvalidParent");
    b.set_list_member(list, string, INHERIT)?;
    assert_eq!(
        kind(b.set_list_member(list, string, INHERIT)),
        "DuplicateMember"
    );
    let (key, value) = entry();
    b.set_map_entry(map, key, value)?;
    let (key, value) = entry();
    assert_eq!(kind(b.set_map_entry(map, key, value)), "DuplicateMember");
    b.add_member(s, "M", string, INHERIT)?;
    assert_eq!(kind(b.add_member(s, "M", text, INHERIT)), "DuplicateMember");
    Ok(())
}

#[test]
fn an_incomplete_list_or_map_is_refused() -> Result<()> {
    for shape_type in [ShapeType::List, ShapeType::Map] {
        let mut b = builder()?;
        b.declare("C", shape_type, None)?;
        assert_eq!(
            b.finish().err(),
            Some(BuildError::InvalidShape("t#C".into()))
        );
    }
    Ok(())
}

#[test]
fn references_target_shapes_never_members_or_overlays() -> Result<()> {
    let mut b = builder()?;
    let string = string(&b)?;
    let s = b.declare("S", ShapeType::Structure, None)?;
    let member = b.add_member(s, "M", string, INHERIT)?;
    let overlay = b.root_overlay(
        s,
        MemberTraits::resolved(KnownTraits::default(), ColdTraits::default()),
    )?;
    assert_eq!(kind(b.add_member(s, "N", member, INHERIT)), "InvalidTarget");
    assert_eq!(
        kind(b.add_member(s, "N", overlay, INHERIT)),
        "InvalidTarget"
    );
    assert_eq!(kind(b.root_overlay(member, INHERIT)), "InvalidTarget");
    assert_eq!(kind(b.root_overlay(overlay, INHERIT)), "InvalidTarget");
    let shapes = OperationShapes {
        input: Some(member),
        ..OperationShapes::default()
    };
    let op = OperationData {
        name: "Op".into(),
        ..OperationData::default()
    };
    assert_eq!(kind(b.add_operation(op, shapes)), "InvalidTarget");
    assert_eq!(b.root_overlay(s, INHERIT)?, s);
    Ok(())
}

#[test]
fn only_named_shapes_take_traits() -> Result<()> {
    let mut b = builder()?;
    let string = string(&b)?;
    let s = b.declare("S", ShapeType::Structure, None)?;
    let member = b.add_member(s, "M", string, INHERIT)?;
    let set = |b: &mut SchemaSetBuilder, id| {
        kind(b.set_traits(id, KnownTraits::default(), ColdTraits::default()))
    };
    assert_eq!(set(&mut b, string), "InvalidTarget");
    assert_eq!(set(&mut b, member), "InvalidTarget");
    assert_eq!(set(&mut b, s), "Ok(())");
    Ok(())
}

#[test]
fn an_operation_added_twice_is_refused() -> Result<()> {
    let mut b = builder()?;
    let op = || OperationData {
        name: "Op".into(),
        ..OperationData::default()
    };
    b.add_operation(op(), OperationShapes::default())?;
    assert_eq!(
        kind(b.add_operation(op(), OperationShapes::default())),
        "DuplicateOperation"
    );
    Ok(())
}

#[test]
fn an_uninterpreted_key_must_sit_where_a_call_can_reach_it() -> Result<()> {
    for site in [
        Site::Operation("Missing".into()),
        Site::Shape("Missing".into()),
        Site::Shape("S$M".into()),
        Site::Shape("String".into()),
    ] {
        let mut b = builder()?;
        let s = b.declare("S", ShapeType::Structure, None)?;
        let string = string(&b)?;
        b.add_member(s, "M", string, INHERIT)?;
        b.record_uninterpreted(site.clone(), "key");
        assert_eq!(kind(b.finish()), "UnknownSite", "{site:?}");
    }
    Ok(())
}

#[test]
fn an_unknown_trait_is_always_recorded() -> Result<()> {
    let mut b = builder()?;
    let s = b.declare("S", ShapeType::Structure, None)?;
    let string = string(&b)?;
    let mut cold = ColdTraits::default();
    let site = Site::Shape("S".into());
    cold.insert_unknown(
        "acme#custom",
        json!({"a": 1}),
        site,
        "members.M.acme#custom",
    )?;
    b.add_member(
        s,
        "M",
        string,
        MemberTraits::resolved(KnownTraits::default(), cold),
    )?;
    let set = b.finish()?;
    let member = set
        .shape_named("S")
        .and_then(|s| s.member("M"))
        .ok_or("no member")?;
    assert_eq!(member.cold_trait("acme#custom"), Some(&json!({"a": 1})));
    assert!(member.has_own_traits());
    let keys: Vec<_> = set
        .uninterpreted()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(keys, ["shape S: members.M.acme#custom"]);
    Ok(())
}

#[test]
fn ids_from_another_builder_are_refused() -> Result<()> {
    let mut other = builder()?;
    for index in 0..100 {
        other.intern(&format!("text {index}"))?;
    }
    let far = other.declare("Far", ShapeType::String, None)?;
    let foreign = other.intern("foreign")?;
    let mut b = builder()?;
    assert_eq!(kind(b.add_member(far, "M", far, INHERIT)), "Foreign");
    let s = b.declare("S", ShapeType::String, None)?;
    let known = KnownTraits {
        json_name: Some(foreign),
        ..KnownTraits::default()
    };
    b.set_traits(s, known, ColdTraits::default())?;
    assert_eq!(b.finish().err(), Some(BuildError::Foreign));
    Ok(())
}

#[test]
fn refusals_become_model_errors() -> Result<()> {
    let missing: ModelError = BuildError::NoShapeFound("t#X".into()).into();
    assert_eq!(missing.to_string(), "Shape not found: t#X");
    let clash: ModelError = BuildError::NameClash("X".into()).into();
    assert_eq!(clash.to_string(), "invalid shape: two shapes named: X");
    assert_eq!(
        ModelError::from(BuildError::TooLarge).to_string(),
        "the model has too many shapes"
    );
    Ok(())
}

#[test]
fn a_member_with_the_targets_traits_shares_them() -> Result<()> {
    let mut b = builder()?;
    let text = b.declare("Text", ShapeType::String, None)?;
    let mut known = KnownTraits::default();
    known.flags.set(Flag::Sensitive, true);
    b.set_traits(text, known.clone(), ColdTraits::default())?;
    let s = b.declare("S", ShapeType::Structure, None)?;
    b.add_member(
        s,
        "Same",
        text,
        MemberTraits::resolved(known.clone(), ColdTraits::default()),
    )?;
    known.flags.set(Flag::Sensitive, false);
    b.add_member(
        s,
        "Cleared",
        text,
        MemberTraits::resolved(known, ColdTraits::default()),
    )?;
    let set = b.finish()?;
    let s = set.shape_named("S").ok_or("no S")?;
    let flags: Vec<_> = s
        .members()
        .map(|m| (m.has_own_traits(), m.flag(Flag::Sensitive)))
        .collect();
    assert_eq!(flags, [(false, true), (true, false)]);
    Ok(())
}
